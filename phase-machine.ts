import type { CommandPhase, CommandStatusPayload, ScramjetState } from "./types.ts";

export const LEGAL_TRANSITIONS: Record<CommandPhase, readonly CommandPhase[]> = {
	idle: ["idle", "running"],
	running: ["idle", "probing", "running"],
	probing: ["idle", "reported", "running"],
	reported: ["idle", "waiting", "running"],
	waiting: ["idle", "running", "waiting"],
};

export function transitionPhase(state: ScramjetState, target: CommandPhase): boolean {
	const from = state.commandPhase;
	if (from === target) return true;
	const allowed = LEGAL_TRANSITIONS[from];
	if (!allowed.includes(target)) {
		console.warn(`[scramjet] illegal phase transition: ${from} → ${target}`);
		return false;
	}
	state.commandPhase = target;
	if (target === "idle") state.latestCommandStatus = null;
	return true;
}

interface PhaseEntry {
	type: "custom";
	customType: string;
	data?: {
		command?: string;
		depth?: number;
		commandName?: string;
		status?: CommandStatusPayload["status"];
	};
}

export function reconstructPhase(entries: readonly PhaseEntry[]): "idle" | "waiting" {
	let activeTopLevelCommand: string | null = null;
	let phase: "idle" | "waiting" = "idle";
	for (const entry of entries) {
		if (entry.type !== "custom") continue;
		if (entry.customType === "scramjet:command-start") {
			const data = entry.data;
			if (!data || typeof data.command !== "string" || data.command === "") continue;
			if (data.depth === 0) {
				activeTopLevelCommand = data.command;
				phase = "idle";
			}
		} else if (entry.customType === "scramjet:command-status") {
			const data = entry.data;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			phase = data.status === "waiting_for_user" ? "waiting" : "idle";
		}
	}
	return phase;
}
