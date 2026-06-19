import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { AgentRegistry, ScramjetState } from "./types.ts";

export function buildAgentCatalogBlock(registry: AgentRegistry): string {
	if (registry.size === 0) return "";
	const entries = [...registry.entries()]
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([name, def]) => (def.description ? `- ${name}: ${def.description}` : `- ${name}`))
		.join("\n");
	return `# Available subagents\n\nThe following agents can be dispatched via the \`subagent\` tool:\n\n${entries}`;
}

export function registerAgentCatalog(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("before_agent_start", () => {
		const block = buildAgentCatalogBlock(state.agentRegistry);
		if (!block) return {};
		return { systemPromptSection: { id: "scramjet:agent-catalog", text: `\n\n${block}` } };
	});
}
