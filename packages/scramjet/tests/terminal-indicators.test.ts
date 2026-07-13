import { describe, expect, it } from "vitest";
import { shouldRingBell, titleForPhase } from "../src/terminal-indicators.js";

describe("shouldRingBell", () => {
	const baseArgs = {
		bellEnabled: true,
		isTTY: true,
		isDispatchScheduled: false,
		isProbeScheduled: false,
		phase: "idle" as string,
		lastBellMs: 0,
		nowMs: 10_000,
	};

	it("returns false when bellEnabled is false", () => {
		expect(shouldRingBell({ ...baseArgs, bellEnabled: false })).toBe(false);
	});

	it("returns false when isTTY is false", () => {
		expect(shouldRingBell({ ...baseArgs, isTTY: false })).toBe(false);
	});

	it("returns false when isDispatchScheduled is true", () => {
		expect(shouldRingBell({ ...baseArgs, isDispatchScheduled: true })).toBe(false);
	});

	it("returns false when isProbeScheduled is true", () => {
		expect(shouldRingBell({ ...baseArgs, isProbeScheduled: true })).toBe(false);
	});

	it("returns false for phase 'running'", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "running" })).toBe(false);
	});

	it("returns false for phase 'probing'", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "probing" })).toBe(false);
	});

	it("returns false for phase 'reported'", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "reported" })).toBe(false);
	});

	it("returns false within 5s cooldown", () => {
		expect(shouldRingBell({ ...baseArgs, lastBellMs: 8_000, nowMs: 10_000 })).toBe(false);
	});

	it("returns true for idle when all guards pass", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "idle" })).toBe(true);
	});

	it("returns true for waiting when all guards pass", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "waiting" })).toBe(true);
	});

	it("returns true for dormant when all guards pass", () => {
		expect(shouldRingBell({ ...baseArgs, phase: "dormant" })).toBe(true);
	});

	it("returns true when exactly at cooldown boundary", () => {
		expect(shouldRingBell({ ...baseArgs, lastBellMs: 5_000, nowMs: 10_000 })).toBe(true);
	});
});

describe("titleForPhase", () => {
	it("uses working prefix for running phase", () => {
		expect(titleForPhase("running", "my-session", "project")).toBe("● scramjet - my-session - project");
	});

	it("uses working prefix for probing phase", () => {
		expect(titleForPhase("probing", "sess", "dir")).toBe("● scramjet - sess - dir");
	});

	it("uses working prefix for reported phase", () => {
		expect(titleForPhase("reported", "s", "d")).toBe("● scramjet - s - d");
	});

	it("uses waiting prefix for idle phase", () => {
		expect(titleForPhase("idle", "my-session", "project")).toBe("○ scramjet - my-session - project");
	});

	it("uses waiting prefix for waiting phase", () => {
		expect(titleForPhase("waiting", "sess", "dir")).toBe("○ scramjet - sess - dir");
	});

	it("uses waiting prefix for dormant phase", () => {
		expect(titleForPhase("dormant", "s", "d")).toBe("○ scramjet - s - d");
	});

	it("omits session name when undefined", () => {
		expect(titleForPhase("idle", undefined, "project")).toBe("○ scramjet - project");
	});

	it("omits session name when undefined for working phase", () => {
		expect(titleForPhase("running", undefined, "mydir")).toBe("● scramjet - mydir");
	});
});
