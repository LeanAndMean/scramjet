import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import { parseDelegateArgs, substituteArguments } from "./commands/substitute.js";
import {
	activeCommandName,
	clearActiveCommand,
	isParkedForInput,
	type LifecycleState,
	reconstructLifecycle,
	resumeFromParkedInput,
	startCommand,
} from "./lifecycle.js";
import type { CommandDef, CommandRegistry, CommandStatusPayload, ScramjetState, SidebarEntry } from "./types.js";

export const COMMAND_START_TYPE = "scramjet:command-start";
export const COMMAND_STATUS_TYPE = "scramjet:command-status";
export const USER_INPUT_PARKED_TYPE = "scramjet:user-input-parked";
export const COMMAND_EXIT_TYPE = "scramjet:command-exited";
export const ENABLED_TOGGLE_TYPE = "scramjet:enabled-toggle";
export const SIDEBAR_MAX = 50;

// Pi built-ins that the F25 clear-on-unknown-slash path must NOT treat as
// a workflow exit. Used only when pi.getCommands() is unavailable (older Pi,
// test fakes that don't stub it); the normal path consults the live command
// list. (F4)
const FALLBACK_KNOWN_SLASH = new Set<string>(["autopilot", "clear", "scramjet"]);

function extractSlashName(text: string): string | null {
	if (!text.startsWith("/")) return null;
	const rest = text.slice(1);
	const space = rest.search(/\s/);
	const name = space === -1 ? rest : rest.slice(0, space);
	return name || null;
}

function isKnownSlashCommand(text: string, pi: ExtensionAPI, state: ScramjetState): boolean {
	const name = extractSlashName(text);
	if (!name) return false;
	const getCommands = (pi as unknown as { getCommands?: () => Array<{ name: string }> }).getCommands;
	if (typeof getCommands === "function") {
		try {
			for (const cmd of getCommands()) {
				if (cmd.name === name) return true;
			}
			return false;
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			state.logger.warn("history", "slash command lookup failed; preserving active Scramjet workflow", {
				slashName: name,
				error: message,
			});
			return true;
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

// Extracts everything after the first whitespace character in a slash command.
export function extractArgs(text: string): string {
	const idx = text.search(/\s/);
	return idx === -1 ? "" : text.slice(idx + 1);
}

// Substitutes arguments into a command body and wraps in <scramjet-command> tags.
export function buildCommandExpansion(name: string, def: CommandDef, argsString: string): string {
	const parsedArgs = parseDelegateArgs(argsString);
	const body = substituteArguments(def.body, parsedArgs);
	if (body.startsWith("<scramjet-command")) return body;
	return `<scramjet-command name="${name}">\n${body}\n</scramjet-command>`;
}

// Pure push + trim-to-SIDEBAR_MAX from the right. Returns a new array.
export function appendSidebarEntry(log: SidebarEntry[], entry: SidebarEntry): SidebarEntry[] {
	const next = [...log, entry];
	return next.length > SIDEBAR_MAX ? next.slice(-SIDEBAR_MAX) : next;
}

// Single chokepoint for "a command was invoked." Pushes a sidebar entry and
// persists it to the journal so resume can replay it. Depth-0 entries are
// top-level command starts and update the lifecycle; depth > 0 entries are
// delegated subroutine invocations and must not replace the active top-level
// command whose next-step policy controls the turn.

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
		state.clearLifecycleTimers?.();
		const result = startCommand(state, name);
		if (!result.ok) {
			state.logger.warn("lifecycle", `lifecycle startCommand failed: ${result.reason}`, {
				event: "command-start",
				command: name,
			});
			return;
		}
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

// Journal entry for a command-status report (issue 88, issue 278). Records which
// command reported, what status, and the incremental work summary. Every accepted
// report is journaled — including "continuing" — so summaries form a searchable
// artifact trail. Replay only acts on terminal statuses (see VALID_RESTING_STATUSES):
// a completed reconstructs to idle, blocked/incomplete to dormant, continuing is inert.
// Legacy entries written before summaries existed have no `summary` field and remain
// replayable (replay never reads it).
export interface CommandStatusData {
	commandName: string;
	status: CommandStatusPayload["status"];
	summary: string;
}

// Journals the agent's report_scramjet_command_status report. Mirrors
// recordCommandStart's shape (a thin appendEntry wrapper) but mutates no state:
// the live lifecycle facts are owned by command-status.ts / auto-continue.ts; this only
// persists the report so resume can rebuild the resting lifecycle facts. Terminal
// statuses are journaled — that is what lets a command
// which waits, is answered, then completes without offering a next step reconstruct
// to idle instead of resurrecting at dormant (the duplicate-work hazard). "continuing"
// reports are also journaled (issue 278) to preserve incremental summaries; they stay
// replay-inert because VALID_RESTING_STATUSES excludes them.
export function recordCommandStatus(
	pi: ExtensionAPI,
	commandName: string,
	status: CommandStatusPayload["status"],
	summary: string,
): void {
	const data: CommandStatusData = { commandName, status, summary };
	pi.appendEntry(COMMAND_STATUS_TYPE, data);
}

export interface ReplayResult {
	sidebarLog: SidebarEntry[];
	// null when no toggle entry was found on the replayed branch — caller
	// preserves its prior value rather than resetting to the default.
	enabled: boolean | null;
	lifecycle: LifecycleState;
}

const VALID_RESTING_STATUSES: ReadonlySet<string> = new Set(["completed", "blocked", "incomplete"]);

export function replayHistory(entries: readonly SessionEntry[]): ReplayResult {
	let sidebarLog: SidebarEntry[] = [];
	let enabled: boolean | null = null;
	let activeTopLevelCommand: string | null = null;
	let parkedForInput = false;
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
				parkedForInput = false;
			}
		} else if (entry.customType === ENABLED_TOGGLE_TYPE) {
			const data = entry.data as EnabledToggleData | undefined;
			if (data && typeof data.enabled === "boolean") enabled = data.enabled;
		} else if (entry.customType === COMMAND_STATUS_TYPE) {
			const data = entry.data as { commandName?: unknown; status?: unknown } | undefined;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			if (typeof data.status !== "string" || !VALID_RESTING_STATUSES.has(data.status)) continue;
			if (data.status === "completed") {
				activeTopLevelCommand = null;
			}
			// blocked/incomplete: command stays associated (dormant)
			parkedForInput = false;
		} else if (entry.customType === USER_INPUT_PARKED_TYPE) {
			const data = entry.data as { commandName?: unknown; parked?: unknown } | undefined;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			// parked: false is the consumed-reply outcome — it clears waiting for the
			// active command. A missing field is a legacy park entry (means parked).
			// Any other value is malformed and inert.
			if (data.parked === false) {
				parkedForInput = false;
			} else if (data.parked === undefined || data.parked === true) {
				parkedForInput = true;
			}
		} else if (entry.customType === COMMAND_EXIT_TYPE) {
			const data = entry.data as { commandName?: unknown } | undefined;
			if (!data || typeof data.commandName !== "string" || data.commandName === "") continue;
			if (data.commandName !== activeTopLevelCommand) continue;
			activeTopLevelCommand = null;
			parkedForInput = false;
		}
	}
	const lifecycle = reconstructLifecycle(activeTopLevelCommand, parkedForInput);
	return { sidebarLog, enabled, lifecycle };
}

export function registerHistory(pi: ExtensionAPI, state: ScramjetState): void {
	const rebuild = async (_event: unknown, ctx: ExtensionContext) => {
		state.clearLifecycleTimers?.();
		const result = replayHistory(ctx.sessionManager.getBranch());
		state.sidebarLog = result.sidebarLog;
		state.lifecycle = result.lifecycle;
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
		state.pendingSuggestion = null;
		state.freetextAwaitingReply = false;
	});

	pi.on("input", async (event) => {
		state.pendingSuggestion = null;
		state.freetextAwaitingReply = false;
		const name = parseSlashCommand(event.text, state.registry);
		if (!name) {
			// Resume a parked command on an interactive non-slash reply. Only
			// parked-for-input (freetext park) auto-resumes; dormant commands
			// require the agent to explicitly call `continuing` after seeing
			// the dormant notice (issue 215).
			if (event.source === "interactive" && !event.text.startsWith("/") && isParkedForInput(state.lifecycle)) {
				const command = activeCommandName(state.lifecycle);
				const result = resumeFromParkedInput(state);
				// Record the consumed park only when the mutation actually cleared
				// waiting, so replay reconstructs dormant rather than waiting. Never
				// persist the reply text.
				if (result.ok && command) {
					pi.appendEntry(USER_INPUT_PARKED_TYPE, { commandName: command, parked: false });
				}
				return;
			}
			// A slash command that didn't resolve to anything in the registry
			// (typo, removed command, stale alias) is a strong signal the user
			// has moved on from any active workflow. Exit the workflow
			// so the next agent_end doesn't apply the *previous* command's
			// next-step policy to whatever the agent does in response. (F25)
			//
			// Exception: a slash command that *is* registered with Pi (built-ins
			// like /autopilot, /clear, /help, plus other extensions' commands) is
			// not a workflow exit — the user toggling /autopilot on mid-chain or
			// checking /help should not silently break a forced next-step. Only
			// truly unrecognized slashes (typos, removed commands) clear. (F4)
			if (event.text.startsWith("/") && activeCommandName(state.lifecycle) !== null) {
				if (!isKnownSlashCommand(event.text, pi, state)) {
					const command = activeCommandName(state.lifecycle);
					state.clearLifecycleTimers?.();
					const result = clearActiveCommand(state, "unknown-slash");
					// Record the exit only when the active command was actually cleared,
					// so replay reconstructs idle rather than dormant.
					if (result.ok && command) {
						pi.appendEntry(COMMAND_EXIT_TYPE, { commandName: command });
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
		// Compute the transform before recording the start — recordCommandStart
		// mutates lifecycle state, so we must not leave it inconsistent if
		// expansion fails.
		const def = state.registry.get(name);
		if (!def) return;
		const argsString = extractArgs(event.text);
		const wrapped = buildCommandExpansion(name, def, argsString);

		recordCommandStart(pi, state, name, origin);
		return { action: "transform" as const, text: wrapped };
	});
}
