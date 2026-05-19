import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SIDEBAR_MAX } from "./history.ts";
import type { ScramjetState, SidebarEntry } from "./types.ts";

const NO_OUTPUT_TEXT = "(no output)";

// Watches `tool_result` for the upstream subagent example tool returning a
// literal `(no output)` payload on its success path (single mode and chain
// mode emit this when the spawned subprocess exits 0 but produces no
// assistant text — typically a crash before stdout flush, an unknown agent
// model id, or a config error that left messages empty). Without this hook
// the calling agent silently receives "(no output)" as the tool result and
// the operator has no signal that a lens dropped. Advisory only: we never
// modify the tool result content; the calling agent still receives whatever
// upstream produced.
export function registerSubagentOutputAdvisor(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("tool_result", async (event) => {
		if (event.toolName !== "subagent") return;
		const parts = event.content;
		if (!Array.isArray(parts) || parts.length !== 1) return;
		const part = parts[0];
		if (part.type !== "text" || typeof part.text !== "string") return;
		if (part.text.trim() !== NO_OUTPUT_TEXT) return;

		const input = event.input as { agent?: unknown; chain?: unknown };
		let agentName = "<unknown>";
		if (typeof input.agent === "string" && input.agent.trim() !== "") {
			agentName = input.agent;
		} else if (Array.isArray(input.chain) && input.chain.length > 0) {
			const last = input.chain[input.chain.length - 1] as { agent?: unknown };
			if (last && typeof last.agent === "string") agentName = `chain ending in ${last.agent}`;
		}

		console.warn(
			`[scramjet] advisory: subagent '${agentName}' returned no output (subprocess exited cleanly with no assistant text; check ~/.pi/agent/agents/ for the agent definition and rerun with verbose output if reproducing)`,
		);

		const entry: SidebarEntry = {
			command: `subagent ${agentName}: no output`,
			origin: "agent",
			depth: state.delegateStack.length,
			timestamp: Date.now(),
		};
		const next = [...state.sidebarLog, entry];
		state.sidebarLog = next.length > SIDEBAR_MAX ? next.slice(-SIDEBAR_MAX) : next;
	});
}
