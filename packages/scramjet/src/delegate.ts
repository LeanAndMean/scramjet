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
	error?: "unknown_command" | "cycle";
	chain?: string;
}

interface DelegateSuccessDetails {
	command: string;
	depth: number;
	effectiveAllowedTools?: string[];
}

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
		name: "delegate",
		label: "Delegate",
		description:
			"Invoke another scramjet-registered command as a subroutine. The command's body is returned as text in this tool's result; read it and follow its instructions inside the same conversation context. The `args` string is substituted into $ARGUMENTS, $@, and $1-$N within the body (plus bash-style slicing), mirroring Pi's slash-command argument expansion. Cycle detection rejects re-entering a command already on the call stack for this turn.",
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
			const command = typeof args?.command === "string" && args.command.length > 0 ? `/${args.command}` : "delegate";
			const compactArgs = renderedArgs(args?.args);
			const invocation = compactArgs ? `${command} ${compactArgs}` : command;
			return new Text(
				theme.fg("toolTitle", theme.bold("delegate")) +
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
			const def = state.registry.get(params.command);
			if (!def) {
				const details: DelegateDetails = { error: "unknown_command", command: params.command };
				return {
					content: [
						{
							type: "text",
							text: `ERROR: unknown command '${params.command}'. Check the registry or fix the name.`,
						},
					],
					details,
				};
			}
			if (detectCycle(state.delegateStack, params.command)) {
				const chain = [...state.delegateStack.map((f) => f.commandName), params.command].join(" -> ");
				const details: DelegateDetails = { error: "cycle", command: params.command, chain };
				return {
					content: [
						{
							type: "text",
							text: `ERROR: cycle detected in delegation chain ${chain}. Refusing to recurse.`,
						},
					],
					details,
				};
			}
			const activeCommand = activeCommandName(state.lifecycle);
			const callerTools = activeCommand !== null ? state.registry.get(activeCommand)?.allowedTools : undefined;
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
