import type { CommandStatusPayload, CommandStatusRestingStatus } from "./types.ts";

export type LifecycleState =
	| { phase: "idle" }
	| { phase: "dormant"; command: string }
	| { phase: "running"; command: string; continueCount: number }
	| { phase: "probing"; command: string; continueCount: number }
	| { phase: "reported"; command: string; status: CommandStatusPayload; continueCount: number }
	| { phase: "waiting"; command: string };

export type LifecycleEvent =
	| { type: "command-start"; command: string }
	| { type: "agent-end" }
	| { type: "probe-sent" } // Forward placeholder for observability; not currently emitted by any caller.
	| { type: "probe-self-healed" }
	| { type: "status-reported"; status: CommandStatusPayload }
	| { type: "continuing" }
	| { type: "probe-input-resumed" }
	| { type: "terminal-resolved"; status: "completed" | "blocked" | "incomplete" }
	| { type: "waiting-parked" }
	| { type: "user-reply" }
	| { type: "workflow-exit" }
	| { type: "reset" };

export type LifecycleTransitionResult =
	| { ok: true; state: LifecycleState }
	| { ok: false; from: LifecycleState["phase"]; event: LifecycleEvent["type"] };

export type LifecycleInvariantResult = { ok: true } | { ok: false; reason: string };

function hasCommand(command: string): boolean {
	return command.trim() !== "";
}

function hasValidContinueCount(continueCount: number): boolean {
	return Number.isInteger(continueCount) && continueCount >= 0;
}

export function assertInvariant(state: LifecycleState): LifecycleInvariantResult {
	switch (state.phase) {
		case "idle":
			return { ok: true };
		case "dormant":
		case "waiting":
			return hasCommand(state.command) ? { ok: true } : { ok: false, reason: `${state.phase} requires a command` };
		case "running":
		case "probing":
			if (!hasCommand(state.command)) return { ok: false, reason: `${state.phase} requires a command` };
			return hasValidContinueCount(state.continueCount)
				? { ok: true }
				: { ok: false, reason: `${state.phase} requires a non-negative integer continueCount` };
		case "reported":
			if (!hasCommand(state.command)) return { ok: false, reason: "reported requires a command" };
			if (!hasValidContinueCount(state.continueCount)) {
				return { ok: false, reason: "reported requires a non-negative integer continueCount" };
			}
			return state.status.status === "continuing"
				? { ok: false, reason: "reported cannot carry a continuing status" }
				: { ok: true };
	}
}

function ok(from: LifecycleState, event: LifecycleEvent, state: LifecycleState): LifecycleTransitionResult {
	const invariant = assertInvariant(state);
	if (!invariant.ok) return { ok: false, from: from.phase, event: event.type };
	return { ok: true, state };
}

function illegal(state: LifecycleState, event: LifecycleEvent): LifecycleTransitionResult {
	return { ok: false, from: state.phase, event: event.type };
}

export function transition(state: LifecycleState, event: LifecycleEvent): LifecycleTransitionResult {
	if (event.type === "reset") return ok(state, event, { phase: "idle" });
	if (event.type === "command-start") {
		return hasCommand(event.command)
			? ok(state, event, { phase: "running", command: event.command, continueCount: 0 })
			: illegal(state, event);
	}
	if (event.type === "workflow-exit") {
		return state.phase === "idle" ? illegal(state, event) : ok(state, event, { phase: "idle" });
	}

	switch (state.phase) {
		case "idle":
			return illegal(state, event);
		case "dormant":
			if (event.type === "user-reply") {
				return ok(state, event, { phase: "running", command: state.command, continueCount: 0 });
			}
			return illegal(state, event);
		case "running":
			if (event.type === "agent-end") {
				return ok(state, event, { phase: "probing", command: state.command, continueCount: state.continueCount });
			}
			if (event.type === "waiting-parked") return ok(state, event, { phase: "waiting", command: state.command });
			return illegal(state, event);
		case "probing":
			if (event.type === "probe-sent") return ok(state, event, state);
			if (event.type === "probe-self-healed") return ok(state, event, { phase: "dormant", command: state.command });
			if (event.type === "continuing") {
				return ok(state, event, {
					phase: "running",
					command: state.command,
					continueCount: state.continueCount + 1,
				});
			}
			if (event.type === "probe-input-resumed") {
				return ok(state, event, {
					phase: "running",
					command: state.command,
					continueCount: state.continueCount,
				});
			}
			if (event.type === "status-reported") {
				return event.status.status === "continuing"
					? illegal(state, event)
					: ok(state, event, {
							phase: "reported",
							command: state.command,
							status: event.status,
							continueCount: state.continueCount,
						});
			}
			if (event.type === "waiting-parked") return ok(state, event, { phase: "waiting", command: state.command });
			return illegal(state, event);
		case "reported":
			if (event.type === "terminal-resolved") return ok(state, event, { phase: "idle" });
			if (event.type === "waiting-parked") return ok(state, event, { phase: "waiting", command: state.command });
			return illegal(state, event);
		case "waiting":
			if (event.type === "user-reply") {
				return ok(state, event, { phase: "running", command: state.command, continueCount: 0 });
			}
			if (event.type === "waiting-parked") return ok(state, event, state);
			return illegal(state, event);
	}
}

export function getActiveCommand(lifecycle: LifecycleState): string | null {
	switch (lifecycle.phase) {
		case "idle":
			return null;
		case "dormant":
		case "running":
		case "probing":
		case "reported":
		case "waiting":
			return lifecycle.command;
	}
}

export interface PhaseEntry {
	type: string;
	customType?: string;
	data?: unknown;
}

const VALID_STATUSES: ReadonlySet<string> = new Set<CommandStatusRestingStatus>([
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
	activeCommandCleared: boolean;
}

export function reconstructPhase(entries: readonly PhaseEntry[]): ReconstructedPhase {
	let activeTopLevelCommand: string | null = null;
	let phase: "idle" | "waiting" = "idle";
	let activeCommandCleared = false;
	for (const entry of entries) {
		if (entry.type !== "custom") continue;
		if (entry.customType === "scramjet:command-start") {
			const data = entry.data as { command?: unknown; depth?: unknown } | undefined;
			if (!data || typeof data.command !== "string" || data.command === "") continue;
			if (data.depth === 0) {
				activeTopLevelCommand = data.command;
				phase = "idle";
				activeCommandCleared = false;
			}
		} else if (entry.customType === "scramjet:command-status") {
			const data = entry.data as { commandName?: unknown; status?: unknown } | undefined;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			if (typeof data.status !== "string" || !VALID_STATUSES.has(data.status)) continue;
			phase = data.status === "waiting_for_user" ? "waiting" : "idle";
			activeCommandCleared = data.status !== "waiting_for_user";
		}
	}
	return { phase, activeCommandCleared };
}
