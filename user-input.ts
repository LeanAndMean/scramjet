import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { ScramjetState } from "./types.ts";

const ALLOWED_PHASES = new Set(["running", "probing"]);

const OUT_OF_PHASE_ERROR =
	"scramjet_user_input is not available right now. " +
	"This tool can only be called during active command execution (running or probing phase).";

const NON_TUI_ERROR =
	"scramjet_user_input requires a TUI environment. " +
	"The current session does not support interactive UI — use prose-based interaction instead.";

const PROMPT_SNIPPET =
	"You have access to `scramjet_user_input` for requesting structured user input mid-turn " +
	"(confirm, select, or freetext). The tool blocks until the user responds and returns their " +
	"answer as the tool result — the turn does not end. Use it when you need explicit user " +
	"decisions during command execution rather than ending the turn with a prose question.";

export function registerUserInputTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "scramjet_user_input",
		label: "Scramjet User Input",
		description:
			"Request structured input from the user during command execution. " +
			"Supports confirm (yes/no), select (pick from options), and freetext (open-ended input). " +
			"Blocks until the user responds; the turn continues after the response.",
		promptSnippet: PROMPT_SNIPPET,
		parameters: Type.Object({
			type: Type.Union([Type.Literal("confirm"), Type.Literal("select"), Type.Literal("freetext")], {
				description: "The interaction type: confirm, select, or freetext.",
			}),
			message: Type.String({ description: "The question or prompt to show the user." }),
			options: Type.Optional(
				Type.Array(
					Type.Object({
						value: Type.String(),
						label: Type.String(),
						description: Type.Optional(Type.String()),
					}),
					{ description: "Required for select type. The list of options to present." },
				),
			),
			recommended: Type.Optional(
				Type.Integer({
					minimum: 0,
					description: "For select type: zero-based index of the recommended option.",
				}),
			),
			placeholder: Type.Optional(
				Type.String({ description: "For freetext type: placeholder hint text shown in the input." }),
			),
		}),
		async execute(_toolCallId, params, _resource, _read, ctx) {
			if (!ALLOWED_PHASES.has(state.commandPhase)) {
				console.warn(`scramjet: scramjet_user_input called out of phase (phase=${state.commandPhase}); rejected`);
				return {
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
					details: { error: "out-of-phase", phase: state.commandPhase },
				};
			}

			if (!ctx?.ui) {
				return {
					content: [{ type: "text", text: NON_TUI_ERROR }],
					details: { error: "non-tui" },
				};
			}

			const validationError = validateParams(params);
			if (validationError) {
				return {
					content: [{ type: "text", text: validationError }],
					details: { error: "validation", message: validationError },
				};
			}

			// Stub: actual UI interactions wired in Stage 2
			return {
				content: [{ type: "text", text: "scramjet_user_input: UI interactions not yet implemented." }],
				details: { error: "not-implemented" },
			};
		},
	});
}

function validateParams(params: {
	type: string;
	message?: string;
	options?: unknown[];
	recommended?: number;
}): string | null {
	if (!params.message || params.message.trim() === "") {
		return "Validation error: 'message' is required and must be non-empty.";
	}

	if (params.type === "select") {
		if (!Array.isArray(params.options) || params.options.length === 0) {
			return "Validation error: 'options' is required and must be a non-empty array for select type.";
		}
		if (params.recommended !== undefined && (params.recommended < 0 || params.recommended >= params.options.length)) {
			return `Validation error: 'recommended' index ${params.recommended} is out of range (0-${params.options.length - 1}).`;
		}
	}

	return null;
}
