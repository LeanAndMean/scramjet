import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createTerminalIndicators, shouldRingBell, titleForPhase } from "../src/terminal-indicators.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
	vi.restoreAllMocks();
	for (const directory of temporaryDirectories.splice(0)) {
		rmSync(directory, { recursive: true, force: true });
	}
});

function indicatorFixture(preferences?: string) {
	const bag = recordingPi();
	bag.pi.getSessionName = () => "session";
	const state = freshState();
	if (preferences !== undefined) {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-terminal-indicators-"));
		temporaryDirectories.push(directory);
		state.preferencesPath = join(directory, "preferences.yaml");
		writeFileSync(state.preferencesPath, preferences);
	}
	const setTitle = vi.fn();
	const setTitleProvider = vi.fn();
	const ctx = { hasUI: true, ui: { setTitle, setTitleProvider } };
	const indicators = createTerminalIndicators(bag.pi, state);
	indicators.register();
	return { ...bag, state, ctx, setTitle, setTitleProvider, indicators };
}

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

describe("createTerminalIndicators", () => {
	it("keeps recovery working between attempts, preserves choices and notifies once after settlement", async () => {
		const fixture = indicatorFixture("title_indicator: true\nbell: true\n");
		const descriptor = Object.getOwnPropertyDescriptor(process.stdout, "isTTY");
		Object.defineProperty(process.stdout, "isTTY", { configurable: true, value: true });
		const restoreTTY = () => {
			if (descriptor) Object.defineProperty(process.stdout, "isTTY", descriptor);
			else Reflect.deleteProperty(process.stdout, "isTTY");
		};
		const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		let resolve!: (value: { status: "completed" }) => void;
		const settlement = new Promise<{ status: "completed" }>((done) => {
			resolve = done;
		});
		const ctx = { ...fixture.ctx, getRunSettlement: () => settlement };
		await fixture.emit("agent_start", {}, ctx);
		await fixture.emit("agent_end", {}, ctx);
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^● scramjet/);
		expect(write).not.toHaveBeenCalled();
		const lease = fixture.indicators.beginChoice(ctx);
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);
		await fixture.emit("agent_end", {}, ctx);
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);
		lease.complete("resume-work");
		fixture.state.lifecycleGeneration++;
		fixture.state.lifecycle = lifecycleFor("idle");
		resolve({ status: "completed" });
		await new Promise((done) => setTimeout(done, 10));
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);
		restoreTTY();
		expect(write).toHaveBeenCalledTimes(1);
	});

	it("does not let a settled observer affect new input", async () => {
		const fixture = indicatorFixture();
		let resolve!: (value: { status: "completed" }) => void;
		const settlement = new Promise<{ status: "completed" }>((done) => {
			resolve = done;
		});
		const ctx = { ...fixture.ctx, getRunSettlement: () => settlement, isIdle: () => true };
		await fixture.emit("agent_start", {}, ctx);
		await fixture.emit("agent_end", {}, ctx);
		await fixture.emit("input", { text: "new work" }, ctx);
		await fixture.emit("agent_start", {}, { ...ctx, getRunSettlement: () => new Promise(() => {}) });
		resolve({ status: "completed" });
		const count = fixture.setTitle.mock.calls.length;
		await new Promise((done) => setTimeout(done, 10));
		expect(fixture.setTitle).toHaveBeenCalledTimes(count);
	});

	it("waits for settlement after idle input and refreshes the title without a stale bell", async () => {
		const fixture = indicatorFixture("title_indicator: true\nbell: true\n");
		const descriptor = Object.getOwnPropertyDescriptor(process.stdout, "isTTY");
		Object.defineProperty(process.stdout, "isTTY", { configurable: true, value: true });
		const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		vi.useFakeTimers();
		try {
			await fixture.emit("session_start", {}, fixture.ctx);
			let resolve!: (value: { status: "completed" }) => void;
			const settlement = new Promise<{ status: "completed" }>((done) => {
				resolve = done;
			});
			const ctx = { ...fixture.ctx, getRunSettlement: () => settlement, isIdle: () => true };
			const provider = fixture.setTitleProvider.mock.calls[0][0] as () => string | undefined;
			await fixture.emit("agent_start", {}, ctx);
			await fixture.emit("agent_end", {}, ctx);
			await fixture.emit("input", { text: "handled locally" }, ctx);
			await vi.runAllTimersAsync();
			expect(provider()).toMatch(/^● scramjet/);
			resolve({ status: "completed" });
			await vi.runAllTimersAsync();
			expect(provider()).toMatch(/^○ scramjet/);
			expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toBe(provider());
			expect(write).not.toHaveBeenCalled();
		} finally {
			vi.useRealTimers();
			if (descriptor) Object.defineProperty(process.stdout, "isTTY", descriptor);
			else Reflect.deleteProperty(process.stdout, "isTTY");
		}
	});

	it("shows waiting during an active choice and resumes the working title", async () => {
		const fixture = indicatorFixture();
		await fixture.emit("session_start", {}, fixture.ctx);
		await fixture.emit("agent_start", {}, fixture.ctx);

		const lease = fixture.indicators.beginChoice(fixture.ctx);
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);

		lease.complete("resume-work");
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^● scramjet/);
	});

	it("waits for every overlapping lease and ignores duplicate completion", async () => {
		const fixture = indicatorFixture();
		await fixture.emit("agent_start", {}, fixture.ctx);
		const first = fixture.indicators.beginChoice(fixture.ctx);
		const second = fixture.indicators.beginChoice(fixture.ctx);

		first.complete("resume-work");
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);
		second.complete("resume-work");
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^● scramjet/);
		const callCount = fixture.setTitle.mock.calls.length;
		second.complete("derive-lifecycle");
		expect(fixture.setTitle).toHaveBeenCalledTimes(callCount);
	});

	it("keeps direct and provider titles consistent", async () => {
		const fixture = indicatorFixture();
		await fixture.emit("session_start", {}, fixture.ctx);
		await fixture.emit("agent_start", {}, fixture.ctx);
		const provider = fixture.setTitleProvider.mock.calls[0][0] as () => string | undefined;

		const lease = fixture.indicators.beginChoice(fixture.ctx);
		expect(provider()).toBe(fixture.setTitle.mock.calls.at(-1)?.[0]);
		lease.complete("resume-work");
		expect(provider()).toBe(fixture.setTitle.mock.calls.at(-1)?.[0]);
	});

	it("derives from lifecycle after a terminating choice until agent end", async () => {
		const fixture = indicatorFixture();
		await fixture.emit("session_start", {}, fixture.ctx);
		await fixture.emit("agent_start", {}, fixture.ctx);
		const provider = fixture.setTitleProvider.mock.calls[0][0] as () => string | undefined;
		const lease = fixture.indicators.beginChoice(fixture.ctx);
		fixture.state.lifecycle = lifecycleFor("dormant");

		lease.complete("derive-lifecycle");
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^○ scramjet/);
		expect(provider()).toMatch(/^○ scramjet/);
		fixture.state.lifecycle = lifecycleFor("running");
		expect(provider()).toMatch(/^● scramjet/);
		await fixture.emit("agent_end", {}, fixture.ctx);
		expect(fixture.setTitle.mock.calls.at(-1)?.[0]).toMatch(/^● scramjet/);
	});

	it.each(["session_start", "session_tree", "agent_start", "agent_end"])(
		"clears stale leases at the %s boundary",
		async (eventName) => {
			const fixture = indicatorFixture();
			await fixture.emit("session_start", {}, fixture.ctx);
			await fixture.emit("agent_start", {}, fixture.ctx);
			fixture.state.lifecycle = lifecycleFor("running");
			const staleLease = fixture.indicators.beginChoice(fixture.ctx);
			const provider = fixture.setTitleProvider.mock.calls.at(-1)?.[0] as () => string | undefined;

			await fixture.emit(eventName, {}, fixture.ctx);
			expect(provider()).toMatch(/^● scramjet/);
			staleLease.complete("derive-lifecycle");
			expect(provider()).toMatch(/^● scramjet/);
		},
	);

	it("clears lifecycle-derived settlement at the agent_start boundary", async () => {
		const fixture = indicatorFixture();
		await fixture.emit("session_start", {}, fixture.ctx);
		await fixture.emit("agent_start", {}, fixture.ctx);
		const provider = fixture.setTitleProvider.mock.calls[0][0] as () => string | undefined;
		const staleLease = fixture.indicators.beginChoice(fixture.ctx);
		fixture.state.lifecycle = lifecycleFor("dormant");
		staleLease.complete("derive-lifecycle");
		expect(provider()).toMatch(/^○ scramjet/);

		await fixture.emit("agent_start", {}, fixture.ctx);
		expect(provider()).toMatch(/^● scramjet/);
	});

	it("does not set titles when the preference is disabled", async () => {
		const fixture = indicatorFixture("title_indicator: false\nbell: false\n");
		await fixture.emit("session_start", {}, fixture.ctx);
		await fixture.emit("agent_start", {}, fixture.ctx);
		const lease = fixture.indicators.beginChoice(fixture.ctx);
		lease.complete("resume-work");

		expect(fixture.setTitle).not.toHaveBeenCalled();
		const provider = fixture.setTitleProvider.mock.calls[0][0] as () => string | undefined;
		expect(provider()).toBeUndefined();
	});

	it("does not write BEL while choices begin or complete", async () => {
		const fixture = indicatorFixture("title_indicator: true\nbell: true\n");
		const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		await fixture.emit("agent_start", {}, fixture.ctx);

		const lease = fixture.indicators.beginChoice(fixture.ctx);
		lease.complete("resume-work");

		expect(write).not.toHaveBeenCalled();
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
