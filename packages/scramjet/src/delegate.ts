import { type ExtensionAPI, keyText } from "@leanandmean/coding-agent";
import { Container, Text } from "@leanandmean/tui";
import { Type } from "typebox";
import { parseDelegateArgs, substituteArguments } from "./commands/substitute.js";
import { recordCommandInvocation } from "./history.js";
import { activeCommandName } from "./lifecycle.js";
import type { DelegateFrame, ScramjetState } from "./types.js";

interface DelegateDetails {
	command: string;
	depth?: number;
	effectiveAllowedTools?: string[];
	error?: "no_active_command" | "report_pending" | "unknown_caller" | "unknown_command" | "not_subcommand" | "cycle";
	chain?: string;
}

interface DelegateSuccessDetails {
	command: string;
	depth: number;
	effectiveAllowedTools?: string[];
}

export const DELEGATE_TOOL_NAME = "delegate";

const MAX_RENDERED_ARGS = 60;

function renderedArgs(args: unknown): string {
	if (typeof args !== "string") return "";
	const singleLine = args.replace(/\s+/g, " ").trim();
	return singleLine.length <= MAX_RENDERED_ARGS ? singleLine : `${singleLine.slice(0, MAX_RENDERED_ARGS)}…`;
}

function textContent(content: unknown): string {
	if (!Array.isArray(content)) return "";
	return content
		.filter((entry): entry is { type: "text"; text: string } => {
			return typeof entry === "object" && entry !== null && entry.type === "text" && typeof entry.text === "string";
		})
		.map((entry) => entry.text)
		.join("\n");
}

function recognizedSuccess(
	result: unknown,
	isPartial: boolean,
	isError: boolean,
): result is {
	content: [{ type: "text"; text: string }];
	details: DelegateSuccessDetails;
} {
	if (isPartial || isError || typeof result !== "object" || result === null) return false;
	const { content, details } = result as { content?: unknown; details?: unknown };
	if (typeof details !== "object" || details === null || Array.isArray(details)) return false;
	if (Object.keys(details).some((key) => !["command", "depth", "effectiveAllowedTools"].includes(key))) return false;
	if (!("command" in details) || typeof details.command !== "string" || details.command.length === 0) return false;
	if (
		!("depth" in details) ||
		typeof details.depth !== "number" ||
		!Number.isInteger(details.depth) ||
		details.depth <= 0
	) {
		return false;
	}
	if ("effectiveAllowedTools" in details) {
		if (!Array.isArray(details.effectiveAllowedTools)) return false;
		if (!details.effectiveAllowedTools.every((tool) => typeof tool === "string")) return false;
	}
	return (
		Array.isArray(content) &&
		content.length === 1 &&
		typeof content[0] === "object" &&
		content[0] !== null &&
		content[0].type === "text" &&
		typeof content[0].text === "string" &&
		content[0].text.trim().length > 0
	);
}

export function detectCycle(stack: DelegateFrame[], commandName: string): boolean {
	return stack.some((f) => f.commandName === commandName);
}

// Returns undefined when both sides are unrestricted. Returns the other
// side when only one is restricted. Returns the set intersection (preserving
// callee's order) when both restrict — an empty array means no tools are
// inside the declared advisory scope, distinct from undefined ("no restriction").
export function intersectTools(caller: string[] | undefined, callee: string[] | undefined): string[] | undefined {
	if (caller === undefined && callee === undefined) return undefined;
	if (caller === undefined) return callee;
	if (callee === undefined) return caller;
	const callerSet = new Set(caller);
	return callee.filter((t) => callerSet.has(t));
}

export function registerDelegateTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: DELEGATE_TOOL_NAME,
		label: "Delegate",
		description:
			"Load a registered delegate-only subcommand's instructions into this conversation for you, the current agent, to execute immediately yourself in the same conversation. Do not use for separate-agent work, ordinary top-level commands, completion routing, or future suggestions. The `args` string is substituted into $ARGUMENTS, $@, and $1-$N within the body (plus bash-style slicing), mirroring Pi's slash-command argument expansion. Cycle detection rejects repeating the active command or a previously loaded subcommand during this turn.",
		promptSnippet: "Load subcommand instructions for the current agent to execute now in the same conversation.",
		promptGuidelines: [
			`Use \`${DELEGATE_TOOL_NAME}\` only to load delegate-only subcommands that you will execute now yourself; use \`subagent\` for separate-agent work and status/suggestion tools for future top-level routing.`,
		],
		parameters: Type.Object({
			command: Type.String({
				description: "The qualified command name to invoke, e.g. 'mach12:push'",
			}),
			args: Type.String({
				description:
					'Argument string (bash-style: whitespace-split, single/double quotes group). Pass "" for no arguments.',
			}),
		}),
		renderCall(args, theme) {
			const command =
				typeof args?.command === "string" && args.command.length > 0 ? `/${args.command}` : DELEGATE_TOOL_NAME;
			const compactArgs = renderedArgs(args?.args);
			const invocation = compactArgs ? `${command} ${compactArgs}` : command;
			return new Text(
				theme.fg("toolTitle", theme.bold(DELEGATE_TOOL_NAME)) +
					theme.fg("toolOutput", ` ${invocation}`) +
					theme.fg("dim", ` (${keyText("app.tools.expand")} to toggle details)`),
				0,
				0,
			);
		},
		renderResult(result, options, theme, context) {
			const fullText = textContent(result.content);
			if (!recognizedSuccess(result, options.isPartial, context.isError)) {
				const visibleText = fullText || "WARNING: delegate result contains unsupported or malformed content.";
				return new Text(theme.fg(fullText ? "toolOutput" : "warning", visibleText), 0, 0);
			}
			if (options.expanded) return new Text(theme.fg("toolOutput", result.content[0].text), 0, 0);
			if (result.details.effectiveAllowedTools?.length === 0) {
				return new Text(
					theme.fg(
						"warning",
						"WARNING: caller and delegate allowed-tools scopes do not overlap; tool calls are advisory violations. Widen the caller scope or abort delegation.",
					),
					0,
					0,
				);
			}
			return new Container();
		},
		async execute(_id, params) {
			const reject = (error: NonNullable<DelegateDetails["error"]>, text: string, chain?: string) => ({
				content: [{ type: "text" as const, text }],
				details: {
					error,
					command: params.command,
					...(chain === undefined ? {} : { chain }),
				} satisfies DelegateDetails,
			});
			const activeCommand = activeCommandName(state.lifecycle);
			if (activeCommand === null) {
				return reject(
					"no_active_command",
					"ERROR: delegate requires an active command. Start the top-level command before loading a subcommand.",
				);
			}
			if (state.lifecycle.lastReport !== null) {
				return reject(
					"report_pending",
					"ERROR: terminal command status is pending dispatch; no more subcommand work may start.",
				);
			}
			const caller = state.registry.get(activeCommand);
			if (!caller) {
				return reject(
					"unknown_caller",
					`ERROR: active command '${activeCommand}' is absent from the live registry. Restart the command before loading a subcommand.`,
				);
			}
			const def = state.registry.get(params.command);
			if (!def) {
				return reject(
					"unknown_command",
					`ERROR: unknown command '${params.command}'. Check the registry or fix the name.`,
				);
			}
			if (!def.delegateOnly) {
				return reject(
					"not_subcommand",
					`ERROR: '${params.command}' is an ordinary top-level command, not a delegate-only subcommand. Use slash dispatch or the active caller's terminal status routing instead.`,
				);
			}
			if (params.command === activeCommand || detectCycle(state.delegateStack, params.command)) {
				const chain = [activeCommand, ...state.delegateStack.map((f) => f.commandName), params.command].join(
					" -> ",
				);
				return reject("cycle", `ERROR: cycle detected in delegation chain ${chain}. Refusing to recurse.`, chain);
			}
			const callerTools = caller.allowedTools;
			const effectiveAllowedTools = intersectTools(callerTools, def.allowedTools);
			const frame: DelegateFrame = {
				commandName: params.command,
				// Depth is relative to the top-level command shown at depth 0, so the
				// first delegated subroutine is depth 1. Because frames are latched,
				// sequential sibling delegations in one turn still appear at increasing
				// depths until before_agent_start resets the stack.
				depth: state.delegateStack.length + 1,
			};
			if (effectiveAllowedTools !== undefined) frame.effectiveAllowedTools = effectiveAllowedTools;
			state.delegateStack.push(frame);
			recordCommandInvocation(pi, state, params.command, "agent", frame.depth);

			const parsedArgs = parseDelegateArgs(params.args);
			const body = substituteArguments(def.body, parsedArgs);
			// Prepend a visible warning when the declared scopes do not overlap
			// so the agent can widen the caller scope or abort before making
			// tool calls that trigger advisory violations.
			const bodyText =
				effectiveAllowedTools !== undefined && effectiveAllowedTools.length === 0
					? `[scramjet/delegate] WARNING: effective allowed-tools scope for '${params.command}' is empty (caller and callee scopes do not overlap). Tool calls will trigger advisory warnings rather than be blocked; consider widening the caller's scope or aborting the delegation.\n\n${body}`
					: body;
			const alreadyWrapped = bodyText.startsWith("<scramjet-command");
			const wrappedBody = alreadyWrapped
				? bodyText
				: `<scramjet-command name="${params.command}">\n${bodyText}\n</scramjet-command>`;
			const details: DelegateDetails = { command: params.command, depth: frame.depth };
			if (effectiveAllowedTools !== undefined) details.effectiveAllowedTools = effectiveAllowedTools;
			return {
				content: [{ type: "text", text: wrappedBody }],
				details,
			};
		},
	});

	// Per-turn reset of the latched stack. Frames are pushed on each delegate
	// call and never popped within a turn; the next turn starts with a fresh
	// empty stack regardless of /autopilot on/off. Each delegation independently
	// intersects with the top-level command's scope rather than the previous
	// frame's — true push/pop semantics need a per-frame "delegated body
	// consumed" signal Pi does not currently provide.
	// The advisory warnings that fire when a tool call falls outside the
	// frame's effectiveAllowedTools live in tool-scope-advisory.ts — that's
	// the user-visible surface of this latching.
	// Tracked for post-MVP redesign in issue #34.
	pi.on("before_agent_start", async () => {
		state.delegateStack = [];
	});
}
