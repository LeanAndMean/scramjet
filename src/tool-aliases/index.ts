/**
 * Registers PascalCase Claude Code tool-name aliases (Read, Bash, Edit, Write,
 * Grep, Glob, LS) that delegate to Pi's native lowercase tools. Pi owns the
 * schema, description, and render hooks; scramjet only renames.
 *
 * Plugin agents authored for Claude Code declare `tools: [Read, Bash, ...]`
 * in their frontmatter using PascalCase Claude Code names. Pi's built-in
 * tools use lowercase names. Pi's `--tools` allowlist is a case-sensitive
 * Set, so these PascalCase aliases coexist with the native lowercase tools
 * rather than overriding them.
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

type Factory = (cwd: string) => ToolDefinition<any, any>;

export const CLAUDE_CODE_TOOL_NAMES = ["Read", "Bash", "Edit", "Write", "Grep", "Glob", "LS"] as const;
export type ClaudeCodeToolName = (typeof CLAUDE_CODE_TOOL_NAMES)[number];

// Single source of truth for the Claude Code -> Pi tool wiring. The
// `Record<ClaudeCodeToolName, Factory>` annotation forces every name in
// CLAUDE_CODE_TOOL_NAMES to have a factory; adding a name without a factory
// (or vice versa) is a compile error.
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
		// renderCall, renderResult). The template's execute is discarded: it
		// would bake in process.cwd() at registration time, but Pi's session
		// cwd can move (e.g. on `cd`), so we rebuild per call against ctx.cwd.
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
