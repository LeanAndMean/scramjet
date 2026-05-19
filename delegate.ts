import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { DelegateFrame, ScramjetState } from "./types.ts";

interface DelegateDetails {
	command: string;
	depth?: number;
	effectiveAllowedTools?: string[];
	error?: "unknown_command" | "cycle";
	chain?: string;
}

// Mirrors Pi's parseCommandArgs (core/prompt-templates.js): bash-style
// whitespace split with single/double-quote grouping. Replicated so the
// delegated body expands identically to a /slash invocation.
export function parseDelegateArgs(argsString: string): string[] {
	const args: string[] = [];
	let current = "";
	let inQuote: string | null = null;
	for (let i = 0; i < argsString.length; i++) {
		const char = argsString[i];
		if (inQuote) {
			if (char === inQuote) inQuote = null;
			else current += char;
		} else if (char === '"' || char === "'") {
			inQuote = char;
		} else if (char === " " || char === "\t") {
			if (current) {
				args.push(current);
				current = "";
			}
		} else {
			current += char;
		}
	}
	if (current) args.push(current);
	return args;
}

// Mirrors Pi's substituteArgs. Positional ($1, $2, ...) substituted BEFORE
// wildcards ($@, $ARGUMENTS, ${@:N:L}) so a value containing $<digit>
// doesn't re-substitute. Replacement happens on the template only; values
// are not recursively expanded.
export function substituteArguments(content: string, args: string[]): string {
	let result = content;
	result = result.replace(/\$(\d+)/g, (_, num) => {
		const idx = parseInt(num, 10) - 1;
		return args[idx] ?? "";
	});
	result = result.replace(/\$\{@:(\d+)(?::(\d+))?\}/g, (_, startStr, lengthStr) => {
		let start = parseInt(startStr, 10) - 1;
		if (start < 0) start = 0;
		if (lengthStr) {
			const length = parseInt(lengthStr, 10);
			return args.slice(start, start + length).join(" ");
		}
		return args.slice(start).join(" ");
	});
	const allArgs = args.join(" ");
	result = result.replace(/\$ARGUMENTS/g, allArgs);
	result = result.replace(/\$@/g, allArgs);
	return result;
}

export function detectCycle(stack: DelegateFrame[], commandName: string): boolean {
	return stack.some((f) => f.commandName === commandName);
}

// Returns undefined when both sides are unrestricted. Returns the other
// side when only one is restricted. Returns the set intersection (preserving
// callee's order) when both restrict — an empty array means "no tools allowed,"
// distinct from undefined ("no restriction").
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
			const callerTools =
				state.delegateStack.length > 0
					? state.delegateStack[state.delegateStack.length - 1].effectiveAllowedTools
					: undefined;
			const effectiveAllowedTools = intersectTools(callerTools, def.allowedTools);
			const frame: DelegateFrame = {
				commandName: params.command,
				depth: state.delegateStack.length,
			};
			if (effectiveAllowedTools !== undefined) frame.effectiveAllowedTools = effectiveAllowedTools;
			state.delegateStack.push(frame);

			const parsedArgs = parseDelegateArgs(params.args);
			const body = substituteArguments(def.body, parsedArgs);
			// When the caller/callee allowed-tools intersection is empty, the
			// delegated frame is fully locked: no tool calls will pass the
			// advisory check. Prepend a visible warning so the agent reading
			// the substituted body knows up-front rather than discovering it
			// one denied tool call at a time. Empty array is distinct from
			// undefined ("no restriction"); see intersectTools above.
			const bodyText =
				effectiveAllowedTools !== undefined && effectiveAllowedTools.length === 0
					? `[scramjet/delegate] WARNING: effective allowed-tools scope for '${params.command}' is empty (caller and callee declare disjoint allowed-tools). This delegated frame cannot use any tools; consider widening the caller's scope or aborting the delegation.\n\n${body}`
					: body;
			const details: DelegateDetails = { command: params.command, depth: frame.depth };
			if (effectiveAllowedTools !== undefined) details.effectiveAllowedTools = effectiveAllowedTools;
			return {
				content: [{ type: "text", text: bodyText }],
				details,
			};
		},
	});

	// Per-turn reset of the latched stack. Frames are pushed on each delegate
	// call and never popped within a turn; the next turn starts with a fresh
	// empty stack regardless of /scramjet on/off. Consequence: a second call
	// to the same command in one turn is reported as a cycle, and sequential
	// sibling calls narrow effective tools monotonically rather than each
	// inheriting from the top-level scope. This is the MVP "latched scoping"
	// tradeoff CLAUDE.md names; true push/pop semantics need a per-frame
	// "delegated body consumed" signal Pi does not currently provide.
	// Tracked for post-MVP redesign in issue #34.
	pi.on("before_agent_start", async () => {
		state.delegateStack = [];
	});
}
