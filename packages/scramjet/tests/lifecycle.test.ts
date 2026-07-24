import { describe, expect, it } from "vitest";
import {
	acceptDormantContinuing,
	acceptProbeContinuing,
	acceptTerminalReport,
	activeCommandName,
	armProbe,
	beginProbe,
	CONTINUE_LIMIT,
	canAcceptDormantContinuing,
	canAcceptTerminalReport,
	cancelStructuredInput,
	checkInvariants,
	clearActiveCommand,
	createLifecycle,
	enterDormant,
	hasTerminalReport,
	isDormant,
	isParkedForInput,
	isProbeDue,
	isProbeInFlight,
	type LifecycleState,
	type MutationResult,
	parkForFreetext,
	resumeAfterCancelledInput,
	resumeAfterProbeInput,
	resumeFromParkedInput,
	startCommand,
} from "../src/lifecycle.js";
import type { CommandStatusRestingPayload } from "../src/types.js";
import { freshLifecycleHolder } from "./helpers.js";

function ok(result: MutationResult): void {
	expect(result).toEqual({ ok: true });
}

function fails(result: MutationResult, reason?: string): void {
	expect(result.ok).toBe(false);
	if (reason && !result.ok) {
		expect(result.reason).toContain(reason);
	}
}

const completedPayload: CommandStatusRestingPayload = { status: "completed", summary: "done" };
const blockedPayload: CommandStatusRestingPayload = { status: "blocked", summary: "missing dep" };
const incompletePayload: CommandStatusRestingPayload = { status: "incomplete", summary: "stopped" };

describe("createLifecycle", () => {
	it("produces a valid initial state", () => {
		const lc = createLifecycle();
		expect(lc.activeCommand).toBeNull();
		expect(lc.probeArmed).toBe(false);
		expect(lc.probeInFlight).toBe(false);
		expect(lc.parkedForInput).toBe(false);
		expect(lc.cancellationResumeEligible).toBe(false);
		expect(lc.continueCount).toBe(0);
		expect(lc.lastReport).toBeNull();
		expect(checkInvariants(lc).ok).toBe(true);
	});
});

describe("checkInvariants", () => {
	it("rejects flags set without active command", () => {
		const cases: Partial<LifecycleState>[] = [
			{ probeArmed: true },
			{ probeInFlight: true },
			{ parkedForInput: true },
			{ continueCount: 1 },
			{ lastReport: completedPayload },
		];
		for (const override of cases) {
			const lc: LifecycleState = { ...createLifecycle(), ...override };
			const result = checkInvariants(lc);
			expect(result.ok).toBe(false);
		}
	});

	it("rejects empty command string", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "  " };
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects multiple mode flags", () => {
		const cases: Partial<LifecycleState>[] = [
			{ activeCommand: "cmd", probeArmed: true, probeInFlight: true },
			{ activeCommand: "cmd", probeArmed: true, parkedForInput: true },
			{ activeCommand: "cmd", probeArmed: true, lastReport: completedPayload },
			{ activeCommand: "cmd", probeInFlight: true, parkedForInput: true },
			{ activeCommand: "cmd", probeInFlight: true, lastReport: completedPayload },
			{ activeCommand: "cmd", parkedForInput: true, lastReport: completedPayload },
		];
		for (const override of cases) {
			const lc: LifecycleState = { ...createLifecycle(), ...override };
			const result = checkInvariants(lc);
			expect(result.ok).toBe(false);
		}
	});

	it("rejects lastReport with status continuing", () => {
		const lc: LifecycleState = {
			...createLifecycle(),
			activeCommand: "cmd",
			lastReport: { status: "continuing" as any, summary: "x" },
		};
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects negative continueCount", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", probeArmed: true, continueCount: -1 };
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects non-integer continueCount", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", probeArmed: true, continueCount: 1.5 };
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects continueCount > 0 when parked", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", parkedForInput: true, continueCount: 1 };
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects continueCount > 0 with lastReport", () => {
		const lc: LifecycleState = {
			...createLifecycle(),
			activeCommand: "cmd",
			lastReport: completedPayload,
			continueCount: 1,
		};
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("rejects continueCount > 0 when dormant", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", continueCount: 1 };
		expect(checkInvariants(lc).ok).toBe(false);
	});

	it("accepts valid armed state with continueCount > 0", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", probeArmed: true, continueCount: 2 };
		expect(checkInvariants(lc).ok).toBe(true);
	});

	it("accepts valid probeInFlight state with continueCount > 0", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd", probeInFlight: true, continueCount: 2 };
		expect(checkInvariants(lc).ok).toBe(true);
	});

	it("accepts valid dormant state", () => {
		const lc: LifecycleState = { ...createLifecycle(), activeCommand: "cmd" };
		expect(checkInvariants(lc).ok).toBe(true);
	});
});

describe("query helpers", () => {
	it("activeCommandName returns the command or null", () => {
		expect(activeCommandName(createLifecycle())).toBeNull();
		expect(activeCommandName({ ...createLifecycle(), activeCommand: "test:cmd" })).toBe("test:cmd");
	});

	it("isDormant", () => {
		expect(isDormant(createLifecycle())).toBe(false);
		expect(isDormant({ ...createLifecycle(), activeCommand: "cmd" })).toBe(true);
		expect(isDormant({ ...createLifecycle(), activeCommand: "cmd", probeArmed: true })).toBe(false);
		expect(isDormant({ ...createLifecycle(), activeCommand: "cmd", probeInFlight: true })).toBe(false);
		expect(isDormant({ ...createLifecycle(), activeCommand: "cmd", parkedForInput: true })).toBe(false);
		expect(isDormant({ ...createLifecycle(), activeCommand: "cmd", lastReport: completedPayload })).toBe(false);
	});

	it("isParkedForInput", () => {
		expect(isParkedForInput(createLifecycle())).toBe(false);
		expect(isParkedForInput({ ...createLifecycle(), activeCommand: "cmd", parkedForInput: true })).toBe(true);
	});

	it("isProbeDue", () => {
		expect(isProbeDue(createLifecycle())).toBe(false);
		expect(isProbeDue({ ...createLifecycle(), activeCommand: "cmd", probeArmed: true })).toBe(true);
		expect(isProbeDue({ ...createLifecycle(), activeCommand: "cmd", probeArmed: true, parkedForInput: true })).toBe(
			false,
		);
	});

	it("isProbeInFlight", () => {
		expect(isProbeInFlight(createLifecycle())).toBe(false);
		expect(isProbeInFlight({ ...createLifecycle(), activeCommand: "cmd", probeInFlight: true })).toBe(true);
	});

	it("hasTerminalReport", () => {
		expect(hasTerminalReport(createLifecycle())).toBe(false);
		expect(hasTerminalReport({ ...createLifecycle(), activeCommand: "cmd", lastReport: completedPayload })).toBe(
			true,
		);
	});

	it("canAcceptTerminalReport requires probeArmed, probeInFlight, or dormant", () => {
		expect(canAcceptTerminalReport(createLifecycle())).toBe(false);
		expect(canAcceptTerminalReport({ ...createLifecycle(), activeCommand: "cmd", probeInFlight: true })).toBe(true);
		expect(canAcceptTerminalReport({ ...createLifecycle(), activeCommand: "cmd", probeArmed: true })).toBe(true);
		expect(canAcceptTerminalReport({ ...createLifecycle(), activeCommand: "cmd", parkedForInput: true })).toBe(false);
		expect(canAcceptTerminalReport({ ...createLifecycle(), activeCommand: "cmd" })).toBe(true);
	});

	it("canAcceptDormantContinuing requires dormant", () => {
		expect(canAcceptDormantContinuing(createLifecycle())).toBe(false);
		expect(canAcceptDormantContinuing({ ...createLifecycle(), activeCommand: "cmd" })).toBe(true);
		expect(canAcceptDormantContinuing({ ...createLifecycle(), activeCommand: "cmd", probeArmed: true })).toBe(false);
	});
});

describe("startCommand", () => {
	it("sets active command and arms probe", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "test:cmd"));
		expect(h.lifecycle.activeCommand).toBe("test:cmd");
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(0);
		expect(h.lifecycleGeneration).toBe(1);
	});

	it("rejects empty command", () => {
		const h = freshLifecycleHolder();
		fails(startCommand(h, ""), "non-empty");
		fails(startCommand(h, "   "), "non-empty");
	});

	it("replaces a previous command", () => {
		const h = freshLifecycleHolder({ activeCommand: "old:cmd", probeInFlight: true, continueCount: 2 });
		ok(startCommand(h, "new:cmd"));
		expect(h.lifecycle.activeCommand).toBe("new:cmd");
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.continueCount).toBe(0);
	});
});

describe("clearActiveCommand", () => {
	it("resets lifecycle to idle", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true, continueCount: 2 });
		ok(clearActiveCommand(h, "completed"));
		expect(h.lifecycle).toEqual(createLifecycle());
		expect(h.lifecycleGeneration).toBe(1);
	});

	it("rejects when already idle", () => {
		const h = freshLifecycleHolder();
		fails(clearActiveCommand(h, "no reason"), "no active command");
	});
});

describe("enterDormant", () => {
	it("clears all flags but keeps command", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true, continueCount: 2 });
		ok(enterDormant(h, "abort"));
		expect(h.lifecycle.activeCommand).toBe("cmd");
		expect(h.lifecycle.probeArmed).toBe(false);
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.continueCount).toBe(0);
		expect(h.lifecycle.lastReport).toBeNull();
	});

	it("works from probeInFlight", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true, continueCount: 1 });
		ok(enterDormant(h, "watchdog"));
		expect(isDormant(h.lifecycle)).toBe(true);
	});

	it("rejects when no active command", () => {
		const h = freshLifecycleHolder();
		fails(enterDormant(h, "test"), "no active command");
	});
});

describe("cancelStructuredInput", () => {
	it.each(["running", "probing"] as const)("atomically grants resumability from %s", (phase) => {
		const h = freshLifecycleHolder({
			activeCommand: "cmd",
			probeArmed: phase === "running",
			probeInFlight: phase === "probing",
		});
		ok(cancelStructuredInput(h));
		expect(isDormant(h.lifecycle)).toBe(true);
		expect(h.lifecycle.cancellationResumeEligible).toBe(true);
		expect(h.lifecycleGeneration).toBe(1);
	});

	it("resumes only eligible dormant commands in one generation bump", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", cancellationResumeEligible: true });
		ok(resumeAfterCancelledInput(h));
		expect(h.lifecycle.cancellationResumeEligible).toBe(false);
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycleGeneration).toBe(1);
	});

	it("rejects eligibility outside exact dormant shape", () => {
		const invalid = {
			...createLifecycle(),
			activeCommand: "cmd",
			probeArmed: true,
			cancellationResumeEligible: true,
		};
		expect(checkInvariants(invalid).ok).toBe(false);
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		fails(resumeAfterCancelledInput(h), "not eligible");
	});

	it("clears eligibility on superseding transitions", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", cancellationResumeEligible: true });
		ok(startCommand(h, "cmd"));
		expect(h.lifecycle.cancellationResumeEligible).toBe(false);
		ok(parkForFreetext(h));
		expect(h.lifecycle.cancellationResumeEligible).toBe(false);
	});
});

describe("armProbe", () => {
	it("arms from dormant", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		ok(armProbe(h, "agent-end"));
		expect(h.lifecycle.probeArmed).toBe(true);
	});

	it("is idempotent when already armed", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		ok(armProbe(h, "redundant"));
	});

	it("rejects when probe is in flight", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true });
		fails(armProbe(h, "x"), "in flight");
	});

	it("rejects when parked", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", parkedForInput: true });
		fails(armProbe(h, "x"), "parked");
	});

	it("rejects when terminal report pending", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", lastReport: completedPayload });
		fails(armProbe(h, "x"), "terminal report");
	});

	it("rejects with no active command", () => {
		const h = freshLifecycleHolder();
		fails(armProbe(h, "x"), "no active command");
	});
});

describe("beginProbe", () => {
	it("transitions armed to in-flight", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true, continueCount: 2 });
		ok(beginProbe(h, "scheduled"));
		expect(h.lifecycle.probeArmed).toBe(false);
		expect(h.lifecycle.probeInFlight).toBe(true);
		expect(h.lifecycle.continueCount).toBe(2);
	});

	it("rejects when not armed", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		fails(beginProbe(h, "x"), "not armed");
	});

	it("rejects with no active command", () => {
		const h = freshLifecycleHolder();
		fails(beginProbe(h, "x"), "no active command");
	});
});

describe("acceptProbeContinuing", () => {
	it("increments counter and re-arms from 0", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true, continueCount: 0 });
		ok(acceptProbeContinuing(h));
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(1);
	});

	it("increments through the limit", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		for (let i = 0; i < CONTINUE_LIMIT; i++) {
			ok(beginProbe(h, "test cycle"));
			ok(acceptProbeContinuing(h));
			expect(h.lifecycle.continueCount).toBe(i + 1);
		}
	});

	it("rejects at the limit", () => {
		const h = freshLifecycleHolder({
			activeCommand: "cmd",
			probeInFlight: true,
			continueCount: CONTINUE_LIMIT,
		});
		fails(acceptProbeContinuing(h), "limit");
	});

	it("rejects when not probing", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		fails(acceptProbeContinuing(h), "no probe in flight");
	});
});

describe("acceptDormantContinuing", () => {
	it("resumes from dormant and resets counter", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		ok(acceptDormantContinuing(h));
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(0);
	});

	it("works after prior continue-limit exhaustion", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		ok(acceptDormantContinuing(h));
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(0);
	});

	it("rejects when not dormant (armed)", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		fails(acceptDormantContinuing(h), "not dormant");
	});

	it("rejects when no active command", () => {
		const h = freshLifecycleHolder();
		fails(acceptDormantContinuing(h), "not dormant");
	});
});

describe("acceptTerminalReport", () => {
	it("stores report and clears probe", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true, continueCount: 2 });
		ok(acceptTerminalReport(h, completedPayload));
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.lastReport).toBe(completedPayload);
		expect(h.lifecycle.continueCount).toBe(0);
	});

	it("accepts blocked and incomplete", () => {
		for (const payload of [blockedPayload, incompletePayload]) {
			const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true });
			ok(acceptTerminalReport(h, payload));
			expect(h.lifecycle.lastReport).toBe(payload);
		}
	});

	it("rejects continuing as terminal", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true });
		fails(acceptTerminalReport(h, { status: "continuing" as any, summary: "x" }), "not a terminal");
	});

	it("accepts from probeArmed (inline) and clears the armed flag", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true, continueCount: 2 });
		ok(acceptTerminalReport(h, completedPayload));
		expect(h.lifecycle.probeArmed).toBe(false);
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.lastReport).toBe(completedPayload);
		expect(h.lifecycle.continueCount).toBe(0);
		expect(isProbeDue(h.lifecycle)).toBe(false);
	});

	it("rejects from non-qualifying state (parked)", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", parkedForInput: true });
		fails(acceptTerminalReport(h, completedPayload), "not running, probing, or dormant");
	});

	it("accepts from dormant state", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd" });
		ok(acceptTerminalReport(h, completedPayload));
		expect(h.lifecycle.lastReport).toMatchObject({ status: "completed", summary: "done" });
		expect(h.lifecycle.continueCount).toBe(0);
	});
});

describe("parkForFreetext", () => {
	it("parks and resets counter", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true, continueCount: 2 });
		ok(parkForFreetext(h));
		expect(h.lifecycle.parkedForInput).toBe(true);
		expect(h.lifecycle.probeArmed).toBe(false);
		expect(h.lifecycle.continueCount).toBe(0);
	});

	it("parks from probeInFlight", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true });
		ok(parkForFreetext(h));
		expect(h.lifecycle.parkedForInput).toBe(true);
		expect(h.lifecycle.probeInFlight).toBe(false);
	});

	it("rejects with no active command", () => {
		const h = freshLifecycleHolder();
		fails(parkForFreetext(h), "no active command");
	});
});

describe("resumeFromParkedInput", () => {
	it("clears park and arms probe with counter reset", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", parkedForInput: true });
		ok(resumeFromParkedInput(h));
		expect(h.lifecycle.parkedForInput).toBe(false);
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(0);
	});

	it("rejects when not parked", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		fails(resumeFromParkedInput(h), "not parked");
	});
});

describe("resumeAfterProbeInput", () => {
	it("preserves continueCount", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeInFlight: true, continueCount: 2 });
		ok(resumeAfterProbeInput(h));
		expect(h.lifecycle.probeInFlight).toBe(false);
		expect(h.lifecycle.probeArmed).toBe(true);
		expect(h.lifecycle.continueCount).toBe(2);
	});

	it("rejects when no probe in flight", () => {
		const h = freshLifecycleHolder({ activeCommand: "cmd", probeArmed: true });
		fails(resumeAfterProbeInput(h), "no probe in flight");
	});
});

describe("generation tracking", () => {
	it("bumps generation on every mutation", () => {
		const h = freshLifecycleHolder();
		expect(h.lifecycleGeneration).toBe(0);
		startCommand(h, "cmd");
		expect(h.lifecycleGeneration).toBe(1);
		enterDormant(h, "test");
		expect(h.lifecycleGeneration).toBe(2);
		acceptDormantContinuing(h);
		expect(h.lifecycleGeneration).toBe(3);
		beginProbe(h, "test");
		expect(h.lifecycleGeneration).toBe(4);
		acceptProbeContinuing(h);
		expect(h.lifecycleGeneration).toBe(5);
	});

	it("does not bump generation on rejected mutations", () => {
		const h = freshLifecycleHolder();
		fails(clearActiveCommand(h, "no-op"));
		expect(h.lifecycleGeneration).toBe(0);
		fails(enterDormant(h, "no-op"));
		expect(h.lifecycleGeneration).toBe(0);
	});
});

describe("full engagement cycle", () => {
	it("start → arm → probe → continuing → arm → probe → complete → clear", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "mach12:issue-implement"));
		expect(isProbeDue(h.lifecycle)).toBe(true);

		ok(beginProbe(h, "agent-end"));
		expect(isProbeInFlight(h.lifecycle)).toBe(true);

		ok(acceptProbeContinuing(h));
		expect(h.lifecycle.continueCount).toBe(1);
		expect(isProbeDue(h.lifecycle)).toBe(true);

		ok(beginProbe(h, "agent-end-2"));
		ok(acceptTerminalReport(h, completedPayload));
		expect(hasTerminalReport(h.lifecycle)).toBe(true);
		expect(h.lifecycle.lastReport?.status).toBe("completed");

		ok(clearActiveCommand(h, "dispatched"));
		expect(h.lifecycle).toEqual(createLifecycle());
	});

	it("start → park → resume → probe → complete", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "cmd"));
		ok(parkForFreetext(h));
		expect(isParkedForInput(h.lifecycle)).toBe(true);

		ok(resumeFromParkedInput(h));
		expect(isProbeDue(h.lifecycle)).toBe(true);

		ok(beginProbe(h, "post-resume"));
		ok(acceptTerminalReport(h, completedPayload));
		ok(clearActiveCommand(h, "done"));
	});

	it("start → probe → dormant (self-heal) → dormant continuing → probe → complete", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "cmd"));
		ok(beginProbe(h, "scheduled"));
		ok(enterDormant(h, "watchdog-timeout"));
		expect(isDormant(h.lifecycle)).toBe(true);

		ok(acceptDormantContinuing(h));
		expect(h.lifecycle.continueCount).toBe(0);
		expect(isProbeDue(h.lifecycle)).toBe(true);

		ok(beginProbe(h, "re-armed"));
		ok(acceptTerminalReport(h, completedPayload));
		ok(clearActiveCommand(h, "done"));
	});

	it("continue limit exhaustion → dormant → dormant continuing resumes fresh", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "cmd"));

		for (let i = 0; i < CONTINUE_LIMIT; i++) {
			ok(beginProbe(h, `probe-${i}`));
			ok(acceptProbeContinuing(h));
		}
		expect(h.lifecycle.continueCount).toBe(CONTINUE_LIMIT);

		ok(beginProbe(h, "final"));
		fails(acceptProbeContinuing(h), "limit");

		ok(enterDormant(h, "exhausted"));
		expect(h.lifecycle.continueCount).toBe(0);

		ok(acceptDormantContinuing(h));
		expect(h.lifecycle.continueCount).toBe(0);
		expect(isProbeDue(h.lifecycle)).toBe(true);
	});

	it("start → inline terminal report → clear (no probe)", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "mach12:pr-review"));
		expect(isProbeDue(h.lifecycle)).toBe(true);

		ok(acceptTerminalReport(h, completedPayload));
		expect(isProbeDue(h.lifecycle)).toBe(false);
		expect(isProbeInFlight(h.lifecycle)).toBe(false);
		expect(hasTerminalReport(h.lifecycle)).toBe(true);
		expect(h.lifecycle.lastReport?.status).toBe("completed");

		ok(clearActiveCommand(h, "dispatched"));
		expect(h.lifecycle).toEqual(createLifecycle());
	});

	it("blocked keeps command associated as dormant", () => {
		const h = freshLifecycleHolder();
		ok(startCommand(h, "cmd"));
		ok(beginProbe(h, "sched"));
		ok(acceptTerminalReport(h, blockedPayload));
		expect(h.lifecycle.lastReport?.status).toBe("blocked");
		expect(h.lifecycle.activeCommand).toBe("cmd");
	});
});
