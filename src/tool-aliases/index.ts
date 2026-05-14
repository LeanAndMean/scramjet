/**
 * Registers PascalCase Claude Code tool-name aliases (Read, Bash, Edit, Write,
 * Grep, Glob, LS) that delegate to Pi's native lowercase tools. Pi owns the
 * schema, description, and render hooks; scramjet only renames.
 *
 * Each alias's execute is rebuilt per call with `ctx.cwd` so paths resolve
 * against Pi's session cwd, matching the pattern Pi itself uses internally
 * (createAllToolDefinitions(this._cwd, ...)).
 */

import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import {
	createBashToolDefinition,
	createEditToolDefinition,
	createFindToolDefinition,
	createGrepToolDefinition,
	createLsToolDefinition,
	createReadToolDefinition,
	createWriteToolDefinition,
} from "@earendil-works/pi-coding-agent";
import { CLAUDE_CODE_TOOL_NAMES, type ClaudeCodeToolName } from "./mapping.ts";

type Factory = (cwd: string) => ToolDefinition<any, any>;

const ALIAS_FACTORIES: Record<ClaudeCodeToolName, Factory> = {
	Read: createReadToolDefinition,
	Bash: createBashToolDefinition,
	Edit: createEditToolDefinition,
	Write: createWriteToolDefinition,
	Grep: createGrepToolDefinition,
	Glob: createFindToolDefinition,
	LS: createLsToolDefinition,
};

export function registerToolAliases(pi: ExtensionAPI): void {
	for (const aliasName of CLAUDE_CODE_TOOL_NAMES) {
		const factory = ALIAS_FACTORIES[aliasName];
		// Build template once for cwd-independent fields (schema, description,
		// renderCall, renderResult). The template's execute is discarded.
		const template = factory(process.cwd());
		pi.registerTool({
			...template,
			name: aliasName,
			label: aliasName,
			async execute(toolCallId, params, signal, onUpdate, ctx) {
				return factory(ctx.cwd).execute(toolCallId, params, signal, onUpdate, ctx);
			},
		});
	}
}
