import type { CommandPhase, CommandStatusPayload, ScramjetState } from "./types.ts";

export const LEGAL_TRANSITIONS = {
	idle: ["idle", "running"],
	running: ["idle", "probing", "running"],
	probing: ["idle", "reported", "running"],
	reported: ["idle", "waiting", "running"],
	waiting: ["idle", "running", "waiting"],
} as const satisfies Record<CommandPhase, readonly CommandPhase[]>;

export function transitionPhase(state: ScramjetState, target: CommandPhase): boolean {
	const from = state.commandPhase;
	if (from === target) return true;
	const allowed: readonly CommandPhase[] = LEGAL_TRANSITIONS[from];
	if (!allowed.includes(target)) {
		console.warn(`[scramjet] illegal phase transition: ${from} → ${target}`);
		return false;
	}
	state.commandPhase = target;
	if (target === "idle") state.latestCommandStatus = null;
	return true;
}

export interface PhaseEntry {
	type: string;
	customType?: string;
	data?: unknown;
}

const VALID_STATUSES: ReadonlySet<string> = new Set<CommandStatusPayload["status"]>([
	"completed",
	"waiting_for_user",
	"blocked",
	"incomplete",
]);

export function isPhaseEntry(entry: { type: string; customType?: string; data?: unknown }): entry is PhaseEntry {
	return entry.type === "custom" && typeof entry.customType === "string";
}

export interface ReconstructedPhase {
	phase: "idle" | "waiting";
	activeCommandCompleted: boolean;
}

export function reconstructPhase(entries: readonly PhaseEntry[]): ReconstructedPhase {
	let activeTopLevelCommand: string | null = null;
	let phase: "idle" | "waiting" = "idle";
	let activeCommandCompleted = false;
	for (const entry of entries) {
		if (entry.type !== "custom") continue;
		if (entry.customType === "scramjet:command-start") {
			const data = entry.data as { command?: unknown; depth?: unknown } | undefined;
			if (!data || typeof data.command !== "string" || data.command === "") continue;
			if (data.depth === 0) {
				activeTopLevelCommand = data.command;
				phase = "idle";
				activeCommandCompleted = false;
			}
		} else if (entry.customType === "scramjet:command-status") {
			const data = entry.data as { commandName?: unknown; status?: unknown } | undefined;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			if (typeof data.status !== "string" || !VALID_STATUSES.has(data.status)) continue;
			phase = data.status === "waiting_for_user" ? "waiting" : "idle";
			activeCommandCompleted = data.status === "completed";
		}
	}
	return { phase, activeCommandCompleted };
}
