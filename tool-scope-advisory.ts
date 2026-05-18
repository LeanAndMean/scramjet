import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "./types.ts";

// Advisory-only in the MVP per CLAUDE.md design rationale: log out-of-scope
// tool calls but never block. Hard enforcement (rejecting the tool call at
// the tool_call hook) is deferred until multi-turn save/restore exists so
// the caller's broader scope is restored after a delegated frame returns.
export function registerToolCallAdvisor(pi: ExtensionAPI, state: ScramjetState) {
	pi.on("tool_call", async (event) => {
		if (state.delegateStack.length === 0) return;
		const top = state.delegateStack[state.delegateStack.length - 1];
		if (top.effectiveAllowedTools === undefined) return;
		if (event.toolName === "delegate") return;
		if (top.effectiveAllowedTools.includes(event.toolName)) return;
		console.warn(
			`[scramjet] advisory: tool '${event.toolName}' called inside delegate frame '${top.commandName}' (depth=${top.depth}); not in allowed-tools=[${top.effectiveAllowedTools.join(", ")}]`,
		);
	});
}
