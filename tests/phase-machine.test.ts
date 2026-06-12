import { describe, expect, it, vi } from "vitest";
import { LEGAL_TRANSITIONS, reconstructPhase, transitionPhase } from "../phase-machine.ts";
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
		["probing", "reported"],
		["probing", "idle"],
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
		["running", "waiting"],
		["probing", "waiting"],
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

describe("reconstructPhase", () => {
	function entry(
		customType: string,
		data?: { command?: string; depth?: number; commandName?: string; status?: CommandStatusPayload["status"] },
	) {
		return { type: "custom" as const, customType, data };
	}

	it("returns idle for empty entries", () => {
		expect(reconstructPhase([])).toBe("idle");
	});

	it("returns idle for command-start only", () => {
		expect(reconstructPhase([entry("scramjet:command-start", { command: "a", depth: 0 })])).toBe("idle");
	});

	it("returns waiting for start + waiting_for_user status", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
			]),
		).toBe("waiting");
	});

	it("returns idle for start + completed status", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "completed" }),
			]),
		).toBe("idle");
	});

	it("last-status-wins", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
				entry("scramjet:command-status", { commandName: "a", status: "completed" }),
			]),
		).toBe("idle");
	});

	it("new command start resets phase to idle", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
				entry("scramjet:command-start", { command: "b", depth: 0 }),
			]),
		).toBe("idle");
	});

	it("ignores status entries for non-active command", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "b", status: "waiting_for_user" }),
			]),
		).toBe("idle");
	});

	it("ignores depth > 0 command starts for activeTopLevelCommand", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
				entry("scramjet:command-start", { command: "b", depth: 1 }),
			]),
		).toBe("waiting");
	});

	it("skips malformed command-start entries", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "", depth: 0 }),
				entry("scramjet:command-start", { depth: 0 }),
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
			]),
		).toBe("waiting");
	});

	it("skips malformed command-status entries", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "", status: "waiting_for_user" }),
				entry("scramjet:command-status", { commandName: "a" }),
			]),
		).toBe("idle");
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
		expect(reconstructPhase(entries)).toBe("waiting");
	});
});
