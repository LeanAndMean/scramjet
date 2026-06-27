import type { ScramjetLogger } from "./logger.js";
import type { CommandStatusRestingPayload } from "./types.js";

export interface LifecycleState {
	activeCommand: string | null;
	probeArmed: boolean;
	probeInFlight: boolean;
	parkedForInput: boolean;
	continueCount: number;
	lastReport: CommandStatusRestingPayload | null;
}

export interface LifecycleHolder {
	lifecycle: LifecycleState;
	lifecycleGeneration: number;
	logger: ScramjetLogger;
}

export type MutationResult = { ok: true } | { ok: false; reason: string };

export const CONTINUE_LIMIT = 3;

export function createLifecycle(): LifecycleState {
	return {
		activeCommand: null,
		probeArmed: false,
		probeInFlight: false,
		parkedForInput: false,
		continueCount: 0,
		lastReport: null,
	};
}

export function checkInvariants(lifecycle: LifecycleState): MutationResult {
	if (lifecycle.activeCommand === null) {
		if (lifecycle.probeArmed) return { ok: false, reason: "probeArmed must be false when no active command" };
		if (lifecycle.probeInFlight) return { ok: false, reason: "probeInFlight must be false when no active command" };
		if (lifecycle.parkedForInput) return { ok: false, reason: "parkedForInput must be false when no active command" };
		if (lifecycle.continueCount !== 0) return { ok: false, reason: "continueCount must be 0 when no active command" };
		if (lifecycle.lastReport !== null) return { ok: false, reason: "lastReport must be null when no active command" };
		return { ok: true };
	}

	if (lifecycle.activeCommand.trim() === "") {
		return { ok: false, reason: "activeCommand must be a non-empty string" };
	}

	const modeCount =
		(lifecycle.probeArmed ? 1 : 0) +
		(lifecycle.probeInFlight ? 1 : 0) +
		(lifecycle.parkedForInput ? 1 : 0) +
		(lifecycle.lastReport !== null ? 1 : 0);
	if (modeCount > 1) {
		return {
			ok: false,
			reason: "only one mode flag may be active (probeArmed, probeInFlight, parkedForInput, lastReport)",
		};
	}

	if (lifecycle.lastReport !== null && (lifecycle.lastReport.status as string) === "continuing") {
		return { ok: false, reason: "lastReport.status must not be 'continuing'" };
	}

	if (!Number.isInteger(lifecycle.continueCount) || lifecycle.continueCount < 0) {
		return { ok: false, reason: "continueCount must be a non-negative integer" };
	}

	if (lifecycle.parkedForInput && lifecycle.continueCount !== 0) {
		return { ok: false, reason: "parkedForInput requires continueCount === 0" };
	}
	if (lifecycle.lastReport !== null && lifecycle.continueCount !== 0) {
		return { ok: false, reason: "lastReport requires continueCount === 0" };
	}
	if (
		!lifecycle.probeArmed &&
		!lifecycle.probeInFlight &&
		!lifecycle.parkedForInput &&
		lifecycle.lastReport === null
	) {
		if (lifecycle.continueCount !== 0) {
			return { ok: false, reason: "dormant/idle requires continueCount === 0" };
		}
	}

	return { ok: true };
}

export function derivePhaseLabel(lifecycle: LifecycleState): string {
	if (lifecycle.activeCommand === null) return "idle";
	if (lifecycle.lastReport !== null) return "reported";
	if (lifecycle.probeInFlight) return "probing";
	if (lifecycle.probeArmed) return "running";
	if (lifecycle.parkedForInput) return "waiting";
	return "dormant";
}

// --- Query helpers ---

export function activeCommandName(lifecycle: LifecycleState): string | null {
	return lifecycle.activeCommand;
}

export function isDormant(lifecycle: LifecycleState): boolean {
	return (
		lifecycle.activeCommand !== null &&
		!lifecycle.probeArmed &&
		!lifecycle.probeInFlight &&
		!lifecycle.parkedForInput &&
		lifecycle.lastReport === null
	);
}

export function isParkedForInput(lifecycle: LifecycleState): boolean {
	return lifecycle.activeCommand !== null && lifecycle.parkedForInput;
}

export function isProbeDue(lifecycle: LifecycleState): boolean {
	return lifecycle.activeCommand !== null && lifecycle.probeArmed && !lifecycle.parkedForInput;
}

export function isProbeInFlight(lifecycle: LifecycleState): boolean {
	return lifecycle.activeCommand !== null && lifecycle.probeInFlight;
}

export function hasTerminalReport(lifecycle: LifecycleState): boolean {
	return lifecycle.activeCommand !== null && lifecycle.lastReport !== null;
}

export function canAcceptTerminalReport(lifecycle: LifecycleState): boolean {
	return lifecycle.probeInFlight;
}

export function canAcceptDormantContinuing(lifecycle: LifecycleState): boolean {
	return isDormant(lifecycle);
}

// --- Mutation helpers ---

function bumpAndLog(holder: LifecycleHolder, event: string, detail?: Record<string, unknown>): void {
	holder.lifecycleGeneration++;
	holder.logger.lifecycle(`lifecycle: ${event}`, {
		command: holder.lifecycle.activeCommand ?? "(none)",
		generation: holder.lifecycleGeneration,
		...snapshotFacts(holder.lifecycle),
		...(detail ? { detail } : {}),
	});
}

function snapshotFacts(lifecycle: LifecycleState): Record<string, unknown> {
	return {
		probeArmed: lifecycle.probeArmed,
		probeInFlight: lifecycle.probeInFlight,
		parkedForInput: lifecycle.parkedForInput,
		continueCount: lifecycle.continueCount,
		hasReport: lifecycle.lastReport !== null,
	};
}

function assertPostCondition(lifecycle: LifecycleState, event: string): void {
	const result = checkInvariants(lifecycle);
	if (!result.ok) {
		throw new Error(`lifecycle invariant violated after ${event}: ${result.reason}`);
	}
}

export function startCommand(holder: LifecycleHolder, command: string): MutationResult {
	if (!command || command.trim() === "") {
		return { ok: false, reason: "command must be a non-empty string" };
	}
	holder.lifecycle.activeCommand = command;
	holder.lifecycle.probeArmed = true;
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.parkedForInput = false;
	holder.lifecycle.continueCount = 0;
	holder.lifecycle.lastReport = null;
	bumpAndLog(holder, "startCommand", { command });
	assertPostCondition(holder.lifecycle, "startCommand");
	return { ok: true };
}

export function clearActiveCommand(holder: LifecycleHolder, reason: string): MutationResult {
	if (holder.lifecycle.activeCommand === null) {
		return { ok: false, reason: "no active command to clear" };
	}
	const prev = holder.lifecycle.activeCommand;
	holder.lifecycle.activeCommand = null;
	holder.lifecycle.probeArmed = false;
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.parkedForInput = false;
	holder.lifecycle.continueCount = 0;
	holder.lifecycle.lastReport = null;
	bumpAndLog(holder, "clearActiveCommand", { previousCommand: prev, reason });
	assertPostCondition(holder.lifecycle, "clearActiveCommand");
	return { ok: true };
}

export function enterDormant(holder: LifecycleHolder, reason: string): MutationResult {
	if (holder.lifecycle.activeCommand === null) {
		return { ok: false, reason: "no active command; cannot enter dormant" };
	}
	holder.lifecycle.probeArmed = false;
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.parkedForInput = false;
	holder.lifecycle.continueCount = 0;
	holder.lifecycle.lastReport = null;
	bumpAndLog(holder, "enterDormant", { reason });
	assertPostCondition(holder.lifecycle, "enterDormant");
	return { ok: true };
}

export function armProbe(holder: LifecycleHolder, reason: string): MutationResult {
	if (holder.lifecycle.activeCommand === null) {
		return { ok: false, reason: "no active command; cannot arm probe" };
	}
	if (holder.lifecycle.probeInFlight) {
		return { ok: false, reason: "probe already in flight; cannot arm" };
	}
	if (holder.lifecycle.parkedForInput) {
		return { ok: false, reason: "parked for input; cannot arm probe" };
	}
	if (holder.lifecycle.lastReport !== null) {
		return { ok: false, reason: "terminal report pending; cannot arm probe" };
	}
	holder.lifecycle.probeArmed = true;
	bumpAndLog(holder, "armProbe", { reason });
	assertPostCondition(holder.lifecycle, "armProbe");
	return { ok: true };
}

export function beginProbe(holder: LifecycleHolder, reason: string): MutationResult {
	if (holder.lifecycle.activeCommand === null) {
		return { ok: false, reason: "no active command; cannot begin probe" };
	}
	if (!holder.lifecycle.probeArmed) {
		return { ok: false, reason: "probe not armed; cannot begin" };
	}
	holder.lifecycle.probeArmed = false;
	holder.lifecycle.probeInFlight = true;
	bumpAndLog(holder, "beginProbe", { reason });
	assertPostCondition(holder.lifecycle, "beginProbe");
	return { ok: true };
}

export function acceptProbeContinuing(holder: LifecycleHolder): MutationResult {
	if (!holder.lifecycle.probeInFlight) {
		return { ok: false, reason: "no probe in flight; cannot accept continuing" };
	}
	if (holder.lifecycle.continueCount >= CONTINUE_LIMIT) {
		return { ok: false, reason: `continue limit reached (${CONTINUE_LIMIT})` };
	}
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.probeArmed = true;
	holder.lifecycle.continueCount++;
	bumpAndLog(holder, "acceptProbeContinuing", { continueCount: holder.lifecycle.continueCount });
	assertPostCondition(holder.lifecycle, "acceptProbeContinuing");
	return { ok: true };
}

export function acceptDormantContinuing(holder: LifecycleHolder): MutationResult {
	if (!isDormant(holder.lifecycle)) {
		return { ok: false, reason: "not dormant; cannot accept dormant continuing" };
	}
	holder.lifecycle.probeArmed = true;
	holder.lifecycle.continueCount = 0;
	bumpAndLog(holder, "acceptDormantContinuing");
	assertPostCondition(holder.lifecycle, "acceptDormantContinuing");
	return { ok: true };
}

export function acceptTerminalReport(holder: LifecycleHolder, payload: CommandStatusRestingPayload): MutationResult {
	if (!holder.lifecycle.probeInFlight) {
		return { ok: false, reason: "no probe in flight; cannot accept terminal report" };
	}
	if ((payload.status as string) === "continuing") {
		return { ok: false, reason: "continuing is not a terminal status" };
	}
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.lastReport = payload;
	holder.lifecycle.continueCount = 0;
	bumpAndLog(holder, "acceptTerminalReport", { status: payload.status });
	assertPostCondition(holder.lifecycle, "acceptTerminalReport");
	return { ok: true };
}

export function parkForFreetext(holder: LifecycleHolder): MutationResult {
	if (holder.lifecycle.activeCommand === null) {
		return { ok: false, reason: "no active command; cannot park" };
	}
	holder.lifecycle.probeArmed = false;
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.parkedForInput = true;
	holder.lifecycle.continueCount = 0;
	holder.lifecycle.lastReport = null;
	bumpAndLog(holder, "parkForFreetext");
	assertPostCondition(holder.lifecycle, "parkForFreetext");
	return { ok: true };
}

export function resumeFromParkedInput(holder: LifecycleHolder): MutationResult {
	if (!holder.lifecycle.parkedForInput) {
		return { ok: false, reason: "not parked for input; cannot resume" };
	}
	holder.lifecycle.parkedForInput = false;
	holder.lifecycle.probeArmed = true;
	holder.lifecycle.continueCount = 0;
	bumpAndLog(holder, "resumeFromParkedInput");
	assertPostCondition(holder.lifecycle, "resumeFromParkedInput");
	return { ok: true };
}

export function resumeAfterProbeInput(holder: LifecycleHolder): MutationResult {
	if (!holder.lifecycle.probeInFlight) {
		return { ok: false, reason: "no probe in flight; cannot resume after probe input" };
	}
	holder.lifecycle.probeInFlight = false;
	holder.lifecycle.probeArmed = true;
	bumpAndLog(holder, "resumeAfterProbeInput", { continueCount: holder.lifecycle.continueCount });
	assertPostCondition(holder.lifecycle, "resumeAfterProbeInput");
	return { ok: true };
}
