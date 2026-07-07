import type { ExtensionAPI } from "@leanandmean/coding-agent";
import type { CommandRegistry, ScramjetState } from "./types.js";

export function buildCommandCatalogBlock(registry: CommandRegistry): string {
	const entries = [...registry.values()]
		.filter((def) => !def.delegateOnly)
		.sort((a, b) => a.name.localeCompare(b.name));
	if (entries.length === 0) return "";
	const lines = entries.map((def) => {
		const prefix = def.argumentHint ? `/${def.name} ${def.argumentHint}` : `/${def.name}`;
		return def.description ? `- ${prefix}: ${def.description}` : `- ${prefix}`;
	});
	return `# Available commands\n\nThe following slash commands can be invoked directly:\n\n${lines.join("\n")}`;
}

export function registerCommandCatalog(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("before_agent_start", () => {
		const block = buildCommandCatalogBlock(state.registry);
		if (!block) return {};
		return { systemPromptSection: { id: "scramjet:command-catalog", text: `\n\n${block}` } };
	});
}
