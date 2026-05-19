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
			if (!data) continue;
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
		// Per design decision: leave state.enabled unchanged when the branch
		// has no toggle entry, so the in-memory flag carries across navigation
		// to branches that never explicitly toggled.
		if (result.enabled !== null) state.enabled = result.enabled;
	};

	pi.on("session_start", rebuild);
	pi.on("session_tree", rebuild);

	pi.on("input", async (event) => {
		const name = parseSlashCommand(event.text, state.registry);
		if (!name) return;
		const entry: SidebarEntry = {
			command: name,
			origin: event.source === "interactive" ? "user" : "agent",
			depth: 0,
			timestamp: Date.now(),
		};
		state.activeTopLevelCommand = name;
		state.sidebarLog = appendSidebarEntry(state.sidebarLog, entry);
		pi.appendEntry(COMMAND_START_TYPE, entry);
	});
}
