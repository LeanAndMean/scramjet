/**
 * Claude Code -> Pi tool name mapping.
 *
 * Plugin agents authored for Claude Code declare `tools: [Read, Bash, ...]` in
 * their frontmatter using PascalCase Claude Code names. Pi's built-in tools
 * use lowercase names. Pi's `--tools` allowlist is a case-sensitive Set, so
 * the PascalCase aliases coexist with the native lowercase tools rather than
 * overriding them.
 */

export const CLAUDE_CODE_TOOL_NAMES = ["Read", "Bash", "Edit", "Write", "Grep", "Glob", "LS"] as const;
export type ClaudeCodeToolName = (typeof CLAUDE_CODE_TOOL_NAMES)[number];
export type PiToolName = "read" | "bash" | "edit" | "write" | "grep" | "find" | "ls";

const CLAUDE_TO_PI: Record<ClaudeCodeToolName, PiToolName> = {
	Read: "read",
	Bash: "bash",
	Edit: "edit",
	Write: "write",
	Grep: "grep",
	Glob: "find",
	LS: "ls",
};

export function mapClaudeToolNameToPi(name: string): PiToolName | undefined {
	return (CLAUDE_TO_PI as Record<string, PiToolName>)[name];
}
