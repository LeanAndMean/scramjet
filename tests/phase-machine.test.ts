import { describe, expect, it, vi } from "vitest";
import type { LifecycleEvent, LifecycleState } from "../phase-machine.ts";
import {
	assertInvariant,
	fromLegacy,
	LEGAL_TRANSITIONS,
	reconstructPhase,
	toLegacy,
	transition,
	transitionPhase,
} from "../phase-machine.ts";
import type { CommandPhase, CommandStatusPayload } from "../types.ts";
import { freshState } from "./helpers.ts";

describe("LEGAL_TRANSITIONS", () => {
	it("covers every CommandPhase as a key", () => {
		const phases: CommandPhase[] = ["idle", "running", "probing", "reported", "waiting"];
		for (const p of phases) {
			expect(LEGAL_TRANSITIONS[p]).toBeDefined();
		}
	});
});

describe("transitionPhase", () => {
	const legalPairs: [CommandPhase, CommandPhase][] = [
		["idle", "running"],
		["running", "probing"],
		["running", "idle"],
		["running", "waiting"],
		["probing", "reported"],
		["probing", "idle"],
		["probing", "waiting"],
		["reported", "idle"],
		["reported", "waiting"],
		["reported", "running"],
		["probing", "running"],
		["waiting", "running"],
		["waiting", "idle"],
	];

	for (const [from, to] of legalPairs) {
		it(`allows ${from} → ${to}`, () => {
			const state = freshState({ commandPhase: from });
			expect(transitionPhase(state, to)).toBe(true);
			expect(state.commandPhase).toBe(to);
		});
	}

	it("self-transition idle → idle is a no-op", () => {
		const state = freshState({
			commandPhase: "idle",
			latestCommandStatus: { status: "completed", summary: "x" },
		});
		expect(transitionPhase(state, "idle")).toBe(true);
		expect(state.commandPhase).toBe("idle");
		expect(state.latestCommandStatus).toEqual({ status: "completed", summary: "x" });
	});

	it("self-transition waiting → waiting is a no-op", () => {
		const state = freshState({
			commandPhase: "waiting",
			latestCommandStatus: { status: "waiting_for_user", summary: "y" },
		});
		expect(transitionPhase(state, "waiting")).toBe(true);
		expect(state.commandPhase).toBe("waiting");
		expect(state.latestCommandStatus).toEqual({ status: "waiting_for_user", summary: "y" });
	});

	it("self-transition running → running is a no-op", () => {
		const state = freshState({
			commandPhase: "running",
			latestCommandStatus: { status: "completed", summary: "z" },
		});
		expect(transitionPhase(state, "running")).toBe(true);
		expect(state.commandPhase).toBe("running");
		expect(state.latestCommandStatus).toEqual({ status: "completed", summary: "z" });
	});

	const illegalPairs: [CommandPhase, CommandPhase][] = [
		["idle", "probing"],
		["idle", "reported"],
		["idle", "waiting"],
		["running", "reported"],
		["reported", "probing"],
		["waiting", "probing"],
		["waiting", "reported"],
	];

	for (const [from, to] of illegalPairs) {
		it(`rejects ${from} → ${to}`, () => {
			const state = freshState({ commandPhase: from });
			const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
			expect(transitionPhase(state, to)).toBe(false);
			expect(state.commandPhase).toBe(from);
			expect(warn).toHaveBeenCalledOnce();
			warn.mockRestore();
		});
	}

	it("auto-clears latestCommandStatus on → idle", () => {
		const state = freshState({
			commandPhase: "running",
			latestCommandStatus: { status: "completed", summary: "done" },
		});
		transitionPhase(state, "idle");
		expect(state.latestCommandStatus).toBeNull();
	});

	it("does NOT clear latestCommandStatus on → probing", () => {
		const state = freshState({
			commandPhase: "running",
			latestCommandStatus: { status: "completed", summary: "done" },
		});
		transitionPhase(state, "probing");
		expect(state.latestCommandStatus).toEqual({ status: "completed", summary: "done" });
	});

	it("does NOT clear latestCommandStatus on → running", () => {
		const state = freshState({
			commandPhase: "idle",
			latestCommandStatus: { status: "completed", summary: "done" },
		});
		transitionPhase(state, "running");
		expect(state.latestCommandStatus).toEqual({ status: "completed", summary: "done" });
	});
});

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

describe("toLegacy", () => {
	it("maps idle", () => {
		expect(toLegacy({ phase: "idle" })).toEqual({
			commandPhase: "idle",
			activeTopLevelCommand: null,
			latestCommandStatus: null,
		});
	});

	it("maps dormant to idle with active command", () => {
		expect(toLegacy({ phase: "dormant", command: "cmd" })).toEqual({
			commandPhase: "idle",
			activeTopLevelCommand: "cmd",
			latestCommandStatus: null,
		});
	});

	it("maps running", () => {
		expect(toLegacy({ phase: "running", command: "cmd", continueCount: 2 })).toEqual({
			commandPhase: "running",
			activeTopLevelCommand: "cmd",
			latestCommandStatus: null,
		});
	});

	it("maps probing", () => {
		expect(toLegacy({ phase: "probing", command: "cmd", continueCount: 1 })).toEqual({
			commandPhase: "probing",
			activeTopLevelCommand: "cmd",
			latestCommandStatus: null,
		});
	});

	it("maps reported with status", () => {
		const status: CommandStatusPayload = { status: "completed", summary: "done" };
		expect(toLegacy({ phase: "reported", command: "cmd", status, continueCount: 0 })).toEqual({
			commandPhase: "reported",
			activeTopLevelCommand: "cmd",
			latestCommandStatus: status,
		});
	});

	it("maps waiting", () => {
		expect(toLegacy({ phase: "waiting", command: "cmd" })).toEqual({
			commandPhase: "waiting",
			activeTopLevelCommand: "cmd",
			latestCommandStatus: null,
		});
	});
});

describe("fromLegacy", () => {
	it("maps idle without command to idle", () => {
		expect(fromLegacy({ commandPhase: "idle", activeTopLevelCommand: null, latestCommandStatus: null })).toEqual({
			phase: "idle",
		});
	});

	it("maps idle with command to dormant", () => {
		expect(fromLegacy({ commandPhase: "idle", activeTopLevelCommand: "cmd", latestCommandStatus: null })).toEqual({
			phase: "dormant",
			command: "cmd",
		});
	});

	it("maps running", () => {
		expect(fromLegacy({ commandPhase: "running", activeTopLevelCommand: "cmd", latestCommandStatus: null })).toEqual({
			phase: "running",
			command: "cmd",
			continueCount: 0,
		});
	});

	it("maps probing", () => {
		expect(fromLegacy({ commandPhase: "probing", activeTopLevelCommand: "cmd", latestCommandStatus: null })).toEqual({
			phase: "probing",
			command: "cmd",
			continueCount: 0,
		});
	});

	it("maps reported", () => {
		const status: CommandStatusPayload = { status: "completed", summary: "done" };
		expect(
			fromLegacy({ commandPhase: "reported", activeTopLevelCommand: "cmd", latestCommandStatus: status }),
		).toEqual({ phase: "reported", command: "cmd", status, continueCount: 0 });
	});

	it("maps waiting", () => {
		expect(fromLegacy({ commandPhase: "waiting", activeTopLevelCommand: "cmd", latestCommandStatus: null })).toEqual({
			phase: "waiting",
			command: "cmd",
		});
	});
});

describe("transitionPhase — lifecycle sync", () => {
	it("syncs lifecycle on idle → running", () => {
		const state = freshState({ activeTopLevelCommand: "cmd" });
		transitionPhase(state, "running");
		expect(state.lifecycle).toEqual({ phase: "running", command: "cmd", continueCount: 0 });
	});

	it("syncs lifecycle on running → probing", () => {
		const state = freshState({ commandPhase: "running", activeTopLevelCommand: "cmd" });
		transitionPhase(state, "probing");
		expect(state.lifecycle).toEqual({ phase: "probing", command: "cmd", continueCount: 0 });
	});

	it("syncs lifecycle to dormant on running → idle (activeTopLevelCommand retained)", () => {
		const state = freshState({ commandPhase: "running", activeTopLevelCommand: "cmd" });
		transitionPhase(state, "idle");
		expect(state.activeTopLevelCommand).toBe("cmd");
		expect(state.lifecycle).toEqual({ phase: "dormant", command: "cmd" });
	});

	it("syncs lifecycle to idle on running → idle (no active command)", () => {
		const state = freshState({ commandPhase: "running", activeTopLevelCommand: null });
		transitionPhase(state, "idle");
		expect(state.lifecycle).toEqual({ phase: "idle" });
	});

	it("syncs lifecycle on running → waiting", () => {
		const state = freshState({ commandPhase: "running", activeTopLevelCommand: "cmd" });
		transitionPhase(state, "waiting");
		expect(state.lifecycle).toEqual({ phase: "waiting", command: "cmd" });
	});

	it("does not sync lifecycle on illegal transition", () => {
		const state = freshState({ commandPhase: "idle" });
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		transitionPhase(state, "reported");
		expect(state.lifecycle).toEqual({ phase: "idle" });
		warn.mockRestore();
	});

	it("does not sync lifecycle on self-transition (no-op)", () => {
		const state = freshState({
			commandPhase: "running",
			activeTopLevelCommand: "cmd",
			lifecycle: { phase: "running", command: "cmd", continueCount: 3 },
		});
		transitionPhase(state, "running");
		// Self-transition is a no-op: lifecycle should not be overwritten
		expect(state.lifecycle).toEqual({ phase: "running", command: "cmd", continueCount: 3 });
	});

	it("toLegacy → fromLegacy round-trips for non-dormant states", () => {
		const states: LifecycleState[] = [
			{ phase: "idle" },
			{ phase: "running", command: "cmd", continueCount: 0 },
			{ phase: "probing", command: "cmd", continueCount: 0 },
			{
				phase: "reported",
				command: "cmd",
				status: { status: "completed", summary: "done" },
				continueCount: 0,
			},
			{ phase: "waiting", command: "cmd" },
			{ phase: "dormant", command: "cmd" },
		];
		for (const s of states) {
			expect(fromLegacy(toLegacy(s))).toEqual(s);
		}
	});

	it("toLegacy → fromLegacy loses continueCount > 0 (bridge limitation)", () => {
		const s: LifecycleState = { phase: "running", command: "cmd", continueCount: 3 };
		const roundTripped = fromLegacy(toLegacy(s));
		expect(roundTripped).toEqual({ phase: "running", command: "cmd", continueCount: 0 });
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
