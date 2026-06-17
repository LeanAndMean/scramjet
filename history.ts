import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import {
	fromLegacy,
	getActiveCommand,
	isPhaseEntry,
	type LifecycleState,
	reconstructPhase,
	toLegacy,
	transition,
} from "./phase-machine.ts";
import type { CommandRegistry, CommandStatusRestingStatus, ScramjetState, SidebarEntry } from "./types.ts";

export const COMMAND_START_TYPE = "scramjet:command-start";
export const COMMAND_STATUS_TYPE = "scramjet:command-status";
export const ENABLED_TOGGLE_TYPE = "scramjet:enabled-toggle";
export const SIDEBAR_MAX = 50;

// Pi built-ins that the F25 clear-on-unknown-slash path must NOT treat as
// a workflow exit. Used only when pi.getCommands() is unavailable (older Pi,
// test fakes that don't stub it); the normal path consults the live command
// list. (F4)
const FALLBACK_KNOWN_SLASH = new Set<string>(["scramjet", "clear"]);

function extractSlashName(text: string): string | null {
	if (!text.startsWith("/")) return null;
	const rest = text.slice(1);
	const space = rest.search(/\s/);
	const name = space === -1 ? rest : rest.slice(0, space);
	return name || null;
}

function isKnownSlashCommand(text: string, pi: ExtensionAPI): boolean {
	const name = extractSlashName(text);
	if (!name) return false;
	const getCommands = (pi as unknown as { getCommands?: () => Array<{ name: string }> }).getCommands;
	if (typeof getCommands === "function") {
		try {
			for (const cmd of getCommands()) {
				if (cmd.name === name) return true;
			}
			return false;
		} catch {
			// fall through to fallback set
		}
	}
	return FALLBACK_KNOWN_SLASH.has(name);
}

export interface EnabledToggleData {
	enabled: boolean;
}

// Returns the qualified command name if `text` starts with a registered slash command.
export function parseSlashCommand(text: string, registry: CommandRegistry): string | null {
	const name = extractSlashName(text);
	if (!name) return null;
	return registry.has(name) ? name : null;
}

// Pure push + trim-to-SIDEBAR_MAX from the right. Returns a new array.
export function appendSidebarEntry(log: SidebarEntry[], entry: SidebarEntry): SidebarEntry[] {
	const next = [...log, entry];
	return next.length > SIDEBAR_MAX ? next.slice(-SIDEBAR_MAX) : next;
}

// Single chokepoint for "a command was invoked." Pushes a sidebar entry and
// persists it to the journal so resume can replay it. Depth-0 entries are
// top-level command starts and update activeTopLevelCommand; depth > 0 entries
// are delegated subroutine invocations and must not replace the active top-level
// command whose next-step policy controls the turn.
// Sync legacy fields from lifecycle state. Used during the bridge period
// (Stages 2-6) until legacy fields are removed in Stage 7.
function syncLegacyFromLifecycle(state: ScramjetState): void {
	const legacy = toLegacy(state.lifecycle);
	state.commandPhase = legacy.commandPhase;
	state.activeTopLevelCommand = legacy.activeTopLevelCommand;
	state.latestCommandStatus = legacy.latestCommandStatus;
}

export function recordCommandInvocation(
	pi: ExtensionAPI,
	state: ScramjetState,
	name: string,
	origin: SidebarEntry["origin"],
	depth: number,
): void {
	const entry: SidebarEntry = {
		command: name,
		origin,
		depth,
		timestamp: Date.now(),
	};
	if (depth === 0) {
		const result = transition(state.lifecycle, { type: "command-start", command: name });
		if (!result.ok) {
			console.warn(`[scramjet] illegal lifecycle transition: ${result.from} + command-start`);
			return;
		}
		state.lifecycle = result.state;
		syncLegacyFromLifecycle(state);
		// Bridge: reset the closure counter in command-status.ts until Stage 4
		// migrates it onto the lifecycle variant's continueCount.
		state.resetConsecutiveContinues?.();
	}
	state.sidebarLog = appendSidebarEntry(state.sidebarLog, entry);
	pi.appendEntry(COMMAND_START_TYPE, entry);
}

// Depth-0 convenience wrapper for typed/extension-dispatched slash commands.
// Pi input dispatch makes auto-continued Scramjet commands flow through the
// same input-event handler as user-typed commands.
export function recordCommandStart(
	pi: ExtensionAPI,
	state: ScramjetState,
	name: string,
	origin: SidebarEntry["origin"],
): void {
	recordCommandInvocation(pi, state, name, origin, 0);
}

// Journal entry for a command-status report (issue 88). Records which command
// reported and what status, so a rewind/resume can reconstruct the resumable
// "waiting" lifecycle phase (see replayHistory).
export interface CommandStatusData {
	commandName: string;
	status: CommandStatusRestingStatus;
}

// Journals the agent's report_scramjet_command_status report. Mirrors
// recordCommandStart's shape (a thin appendEntry wrapper) but mutates no state:
// the live phase is owned by command-status.ts / auto-continue.ts; this only
// persists the report so resume can rebuild the resting phase. Terminal/resting
// statuses are journaled, not just waiting_for_user — that is what lets a command
// which waits, is answered, then completes without offering a next step reconstruct
// to "idle" instead of resurrecting at "waiting" (the duplicate-work hazard).
export function recordCommandStatus(pi: ExtensionAPI, commandName: string, status: CommandStatusRestingStatus): void {
	const data: CommandStatusData = { commandName, status };
	pi.appendEntry(COMMAND_STATUS_TYPE, data);
}

export interface ReplayResult {
	sidebarLog: SidebarEntry[];
	// null when no toggle entry was found on the replayed branch — caller
	// preserves its prior value rather than resetting to the default.
	enabled: boolean | null;
	activeTopLevelCommand: string | null;
	// Reconstructed resting lifecycle phase (issue 88). Only the two STABLE
	// resting states are ever reconstructed: "waiting" when the active top-level
	// command's last journaled status was waiting_for_user, otherwise "idle". The
	// transient running/probing/reported phases are never journaled or restored,
	// preserving the issue 84 "phase is not journaled" invariant for everything
	// but the resumable halt.
	phase: "idle" | "waiting";
	lifecycle: LifecycleState;
}

export function replayHistory(entries: readonly SessionEntry[]): ReplayResult {
	let sidebarLog: SidebarEntry[] = [];
	let enabled: boolean | null = null;
	let activeTopLevelCommand: string | null = null;
	for (const entry of entries) {
		if (entry.type !== "custom") continue;
		if (entry.customType === COMMAND_START_TYPE) {
			const data = entry.data as SidebarEntry | undefined;
			// Defend against corrupt or partially-written journal entries: a
			// missing/non-string command would erase activeTopLevelCommand and
			// silently break all subsequent next-step policy lookups. The TS cast
			// above otherwise hides this from the compiler. (F10)
			if (!data || typeof data.command !== "string" || data.command === "") continue;
			sidebarLog = appendSidebarEntry(sidebarLog, data);
			if (data.depth === 0) {
				activeTopLevelCommand = data.command;
			}
		} else if (entry.customType === ENABLED_TOGGLE_TYPE) {
			const data = entry.data as EnabledToggleData | undefined;
			if (data && typeof data.enabled === "boolean") enabled = data.enabled;
		}
	}
	const reconstructed = reconstructPhase(entries.filter(isPhaseEntry));
	if (reconstructed.activeCommandCleared) activeTopLevelCommand = null;
	let lifecycle: LifecycleState;
	if (reconstructed.phase === "waiting" && activeTopLevelCommand) {
		lifecycle = { phase: "waiting", command: activeTopLevelCommand };
	} else if (activeTopLevelCommand && reconstructed.phase === "idle") {
		lifecycle = { phase: "dormant", command: activeTopLevelCommand };
	} else {
		lifecycle = { phase: "idle" };
	}
	return { sidebarLog, enabled, activeTopLevelCommand, phase: reconstructed.phase, lifecycle };
}

export function registerHistory(pi: ExtensionAPI, state: ScramjetState): void {
	const rebuild = async (_event: unknown, ctx: ExtensionContext) => {
		const result = replayHistory(ctx.sessionManager.getBranch());
		state.sidebarLog = result.sidebarLog;
		state.lifecycle = result.lifecycle;
		syncLegacyFromLifecycle(state);
		// pendingForcedDispatch is a transient runtime flag tied to a specific
		// in-flight forced dispatch; it has no meaning after navigation or
		// resume. Clear it explicitly so a stale value (e.g. a forced target
		// that was never resolved because it wasn't in the registry) can't
		// mislabel a future user-typed slash command as origin: "forced". (F18)
		state.pendingForcedDispatch = null;
		// Per design decision: leave state.enabled unchanged when the branch
		// has no toggle entry, so the in-memory flag carries across navigation
		// to branches that never explicitly toggled.
		if (result.enabled !== null) state.enabled = result.enabled;
	};

	// Pi exposes two navigation events: session_start (fresh load or resume)
	// and session_tree (branch switch within a session). The MVP plan
	// originally also named a `session_switch` event, but upstream Pi never
	// shipped it (and an older draft was removed); session_start +
	// session_tree together cover every restore path scramjet cares about.
	pi.on("session_start", rebuild);
	pi.on("session_tree", rebuild);

	// Turn-boundary reset for pendingForcedDispatch: if the input event for a
	// forced dispatch already ran but didn't consume the flag (because the
	// forced target wasn't in the registry, so parseSlashCommand returned
	// null), the agent turn starting is the latest moment we can guarantee
	// the flag is stale. Clearing here prevents the flag from outliving its
	// intended single-turn scope. (F18)
	pi.on("before_agent_start", async () => {
		state.pendingForcedDispatch = null;
	});

	pi.on("input", async (event) => {
		// Bridge: re-sync lifecycle from legacy fields in case an un-migrated
		// consumer (e.g. auto-continue.ts) updated legacy fields without syncing
		// lifecycle. Removed when legacy fields are dropped (Stage 7).
		state.lifecycle = fromLegacy(state);
		const name = parseSlashCommand(event.text, state.registry);
		if (!name) {
			// Resume the active command on an interactive non-slash reply. Two
			// cases: (1) issue 88 — the command reported waiting_for_user and
			// rests at "waiting"; the user's answer re-arms the probe path.
			// (2) issue 128 — the probe self-healed to "idle" but
			// activeTopLevelCommand is still set; the user's reply is still
			// engaging with the command. In both cases, flip to "running" so
			// phase-gated tools (get_scramjet_user_input) work
			// and agent_end fires the running→probing probe. Chaining still
			// requires an explicit completed report, so an off-topic reply can
			// only cause a harmless re-probe, never a chain. Gated on
			// source === "interactive" so the hidden status probe (sent via
			// triggerTurn, which bypasses the input pipeline) and
			// extension-dispatched input can never self-resume.
			if (
				event.source === "interactive" &&
				!event.text.startsWith("/") &&
				(state.lifecycle.phase === "waiting" || state.lifecycle.phase === "dormant")
			) {
				const result = transition(state.lifecycle, { type: "user-reply" });
				if (!result.ok) return;
				state.lifecycle = result.state;
				syncLegacyFromLifecycle(state);
				return;
			}
			// A slash command that didn't resolve to anything in the registry
			// (typo, removed command, stale alias) is a strong signal the user
			// has moved on from any active workflow. Clear activeTopLevelCommand
			// so the next agent_end doesn't apply the *previous* command's
			// next-step policy to whatever the agent does in response. (F25)
			//
			// Exception: a slash command that *is* registered with Pi (built-ins
			// like /scramjet, /clear, /help, plus other extensions' commands) is
			// not a workflow exit — the user toggling /scramjet on mid-chain or
			// checking /help should not silently break a forced next-step. Only
			// truly unrecognized slashes (typos, removed commands) clear. (F4)
			if (event.text.startsWith("/") && getActiveCommand(state.lifecycle) !== null) {
				if (!isKnownSlashCommand(event.text, pi)) {
					const exitResult = transition(state.lifecycle, { type: "workflow-exit" });
					if (exitResult.ok) {
						state.lifecycle = exitResult.state;
						syncLegacyFromLifecycle(state);
					}
				}
			}
			return;
		}
		let origin: SidebarEntry["origin"];
		if (state.pendingForcedDispatch === name) {
			origin = "forced";
			state.pendingForcedDispatch = null;
		} else {
			origin = event.source === "interactive" ? "user" : "agent";
		}
		recordCommandStart(pi, state, name, origin);
	});
}
