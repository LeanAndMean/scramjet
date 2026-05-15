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

// Shape every alias factory must satisfy. The ToolDefinition generics are
// pinned at `<any, any>` (not the narrower `<TSchema, unknown>` default)
// because Pi's ToolDefinition has callback fields -- `renderCall(args:
// Static<TParams>)`, `renderResult(result: AgentToolResult<TDetails>)` --
// that read TParams/TDetails in contravariant positions. Tightening either
// breaks assignment of the per-factory concrete shapes (e.g.
// `ToolDefinition<TObject<{ pattern, path, ... }>, FindToolDetails>`)
// under --strictFunctionTypes. `any` is bivariant and sidesteps the
// variance check.
type Factory = (cwd: string) => ToolDefinition<any, any>;

export const CLAUDE_CODE_TOOL_NAMES = ["Read", "Bash", "Edit", "Write", "Grep", "Glob", "LS"] as const;
export type ClaudeCodeToolName = (typeof CLAUDE_CODE_TOOL_NAMES)[number];

// Single source of truth for the Claude Code -> Pi tool wiring. The
// `Record<ClaudeCodeToolName, Factory>` annotation forces every name in
// CLAUDE_CODE_TOOL_NAMES to have a factory; adding a name without a factory
// (or vice versa) is a compile error.
export const ALIAS_FACTORIES: Record<ClaudeCodeToolName, Factory> = {
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
		//
		// Wrap both factory calls (template-time and per-call) so a Pi-side
		// regression that throws from a factory surfaces with the alias name
		// instead of an opaque stack trace. Without this, a single misbehaving
		// factory would crash registerToolAliases mid-loop and Pi would print
		// the raw error with no hint which alias caused it.
		let template: ToolDefinition<any, any>;
		try {
			template = factory(process.cwd());
		} catch (err) {
			throw new Error(
				`Failed to build alias template for ${aliasName}: ${err instanceof Error ? err.message : String(err)}`,
				{ cause: err },
			);
		}
		pi.registerTool({
			...template,
			name: aliasName,
			label: aliasName,
			async execute(toolCallId, params, signal, onUpdate, ctx) {
				let live: ToolDefinition<any, any>;
				try {
					live = factory(ctx.cwd);
				} catch (err) {
					throw new Error(
						`Failed to build alias instance for ${aliasName} at cwd=${ctx.cwd}: ${err instanceof Error ? err.message : String(err)}`,
						{ cause: err },
					);
				}
				return live.execute(toolCallId, params, signal, onUpdate, ctx);
			},
		});
	}
}
