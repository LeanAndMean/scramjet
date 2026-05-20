import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SIDEBAR_MAX } from "./history.ts";
import type { ScramjetState, SidebarEntry } from "./types.ts";

const NO_OUTPUT_TEXT = "(no output)";

// Resolve a human-readable label for the subagent that produced the
// `(no output)` payload. The upstream subagent tool accepts either
// `{ agent: string }` (single mode) or `{ chain: [{ agent: string }, ...] }`
// (chain mode); both branches reach the no-output success path. Returns
// `<unknown>` when the input doesn't match either shape (defensive: the
// hook still wants to surface the failure even if input shape drifts).
function resolveSubagentLabel(input: unknown): string {
	if (!input || typeof input !== "object") return "<unknown>";
	const candidate = input as { agent?: unknown; chain?: unknown };
	if (typeof candidate.agent === "string" && candidate.agent.trim() !== "") {
		return candidate.agent;
	}
	if (Array.isArray(candidate.chain) && candidate.chain.length > 0) {
		const last = candidate.chain[candidate.chain.length - 1];
		if (
			last &&
			typeof last === "object" &&
			"agent" in last &&
			typeof (last as { agent: unknown }).agent === "string"
		) {
			return `chain ending in ${(last as { agent: string }).agent}`;
		}
	}
	return "<unknown>";
}

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

		const agentName = resolveSubagentLabel(event.input);

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
