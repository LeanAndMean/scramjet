import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { CommandRegistry, ScramjetState, SidebarEntry } from "./types.ts";

export const COMMAND_START_TYPE = "scramjet:command-start";
export const ENABLED_TOGGLE_TYPE = "scramjet:enabled-toggle";
export const SIDEBAR_MAX = 50;

export interface EnabledToggleData {
	enabled: boolean;
}

// Returns the qualified command name if `text` starts with /<name> and <name>
// is registered, else null. The first whitespace-delimited token after the
// slash is the candidate; anything after is treated as args and ignored.
export function parseSlashCommand(text: string, registry: CommandRegistry): string | null {
	if (!text.startsWith("/")) return null;
	const rest = text.slice(1);
	const space = rest.search(/\s/);
	const name = space === -1 ? rest : rest.slice(0, space);
	if (!name) return null;
	return registry.has(name) ? name : null;
}

// Pure push + trim-to-SIDEBAR_MAX from the right. Returns a new array.
export function appendSidebarEntry(log: SidebarEntry[], entry: SidebarEntry): SidebarEntry[] {
	const next = [...log, entry];
	return next.length > SIDEBAR_MAX ? next.slice(-SIDEBAR_MAX) : next;
}

export interface ReplayResult {
	sidebarLog: SidebarEntry[];
	// null when no toggle entry was found on the replayed branch — caller
	// preserves its prior value rather than resetting to the default.
	enabled: boolean | null;
	activeTopLevelCommand: string | null;
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
			if (data.depth === 0) activeTopLevelCommand = data.command;
		} else if (entry.customType === ENABLED_TOGGLE_TYPE) {
			const data = entry.data as EnabledToggleData | undefined;
			if (data && typeof data.enabled === "boolean") enabled = data.enabled;
		}
	}
	return { sidebarLog, enabled, activeTopLevelCommand };
}

export function registerHistory(pi: ExtensionAPI, state: ScramjetState): void {
	const rebuild = async (_event: unknown, ctx: ExtensionContext) => {
		const result = replayHistory(ctx.sessionManager.getBranch());
		state.sidebarLog = result.sidebarLog;
		state.activeTopLevelCommand = result.activeTopLevelCommand;
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
		const name = parseSlashCommand(event.text, state.registry);
		if (!name) {
			// A slash command that didn't resolve to anything in the registry
			// (typo, removed command, stale alias) is a strong signal the user
			// has moved on from any active workflow. Clear activeTopLevelCommand
			// so the next agent_end doesn't apply the *previous* command's
			// next-step policy to whatever the agent does in response. (F25)
			if (event.text.startsWith("/") && state.activeTopLevelCommand !== null) {
				state.activeTopLevelCommand = null;
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
		const entry: SidebarEntry = {
			command: name,
			origin,
			depth: 0,
			timestamp: Date.now(),
		};
		state.activeTopLevelCommand = name;
		state.sidebarLog = appendSidebarEntry(state.sidebarLog, entry);
		pi.appendEntry(COMMAND_START_TYPE, entry);
	});
}
