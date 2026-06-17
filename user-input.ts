import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { type Static, Type } from "typebox";
import { recordCommandStatus } from "./history.ts";
import { MultiLineSelectList } from "./multi-line-select.ts";
import { getActiveCommand, type LifecycleEvent, transition } from "./phase-machine.ts";
import type { ScramjetState } from "./types.ts";

export const USER_INPUT_TYPE = "scramjet:user-input";

const ALLOWED_PHASES = new Set<string>(["running", "probing"]);

const OUT_OF_PHASE_ERROR =
	"get_scramjet_user_input is not available right now. " +
	"This tool can only be called during active command execution (running or probing phase).";

const NON_TUI_ERROR =
	"get_scramjet_user_input requires a TUI environment. " +
	"The current session does not support interactive UI — use prose-based interaction instead.";

const PROMPT_SNIPPET =
	"You have access to `get_scramjet_user_input` for requesting structured user input mid-turn " +
	"(confirm, select, or freetext). Confirm/select block until the user responds and return their " +
	"answer as the tool result; freetext terminates the turn so the user replies in the standard editor. " +
	"State freetext questions in prose before calling the tool.";

const USER_INPUT_OPTION_SCHEMA = Type.Object({
	value: Type.String(),
	label: Type.String(),
	description: Type.Optional(Type.String()),
});

export const USER_INPUT_SCHEMA = Type.Object({
	type: Type.Union([Type.Literal("confirm"), Type.Literal("select"), Type.Literal("freetext")], {
		description: "The interaction type: confirm, select, or freetext.",
	}),
	message: Type.String({ description: "The question or prompt to show the user." }),
	options: Type.Optional(
		Type.Array(USER_INPUT_OPTION_SCHEMA, {
			description: "Required for select type. The list of options to present.",
		}),
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
});

type UserInputParams = Static<typeof USER_INPUT_SCHEMA>;
type UserInputOption = Static<typeof USER_INPUT_OPTION_SCHEMA>;
const _schemaMatchesParams = (params: Static<typeof USER_INPUT_SCHEMA>): UserInputParams => params;
const _paramsMatchSchema = (params: UserInputParams): Static<typeof USER_INPUT_SCHEMA> => params;

export function registerUserInputTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "get_scramjet_user_input",
		label: "Get Scramjet User Input",
		description:
			"Request structured input from the user during command execution. " +
			"Supports confirm (yes/no), select (pick from options), and freetext (open-ended input). " +
			"Confirm and select block until the user responds and return their answer as the tool result; " +
			"freetext terminates the turn so the user replies in the standard editor. " +
			"The placeholder parameter is accepted for compatibility but unused by freetext.",
		promptSnippet: PROMPT_SNIPPET,
		parameters: USER_INPUT_SCHEMA,
		async execute(_toolCallId, params, _resource, _read, ctx) {
			if (!ALLOWED_PHASES.has(state.lifecycle.phase)) {
				console.warn(
					`scramjet: get_scramjet_user_input called out of phase (phase=${state.lifecycle.phase}); rejected`,
				);
				return {
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
					details: { error: "out-of-phase", phase: state.lifecycle.phase },
				};
			}

			const validationError = validateParams(params);
			if (validationError) {
				return {
					content: [{ type: "text", text: validationError }],
					details: { error: "validation", message: validationError },
				};
			}

			if (params.type === "freetext") {
				pi.appendEntry(USER_INPUT_TYPE, {
					interactionType: params.type,
					message: params.message,
				});
				const waitResult = transition(state.lifecycle, { type: "waiting-parked" });
				if (waitResult.ok) {
					state.lifecycle = waitResult.state;
				}
				const activeCommand = getActiveCommand(state.lifecycle);
				if (activeCommand) recordCommandStatus(pi, activeCommand, "waiting_for_user");
				return {
					content: [{ type: "text", text: JSON.stringify({ waiting_for_user: true }) }],
					details: { type: "freetext", waiting_for_user: true },
					terminate: true,
				};
			}

			if (!ctx?.ui) {
				return {
					content: [{ type: "text", text: NON_TUI_ERROR }],
					details: { error: "non-tui" },
				};
			}

			const isProbing = state.lifecycle.phase === "probing";
			if (isProbing) state.suspendProbeWatchdog?.();

			type InteractionResult = {
				content: { type: "text"; text: string }[];
				details: Record<string, unknown>;
				cancelled: boolean;
			};
			let result: InteractionResult | undefined;
			try {
				switch (params.type) {
					case "confirm":
						result = await handleConfirm(params.message, ctx as ExtensionContext);
						break;
					case "select":
						result = await handleSelect(
							params.message,
							params.options ?? [],
							params.recommended,
							ctx as ExtensionContext,
						);
						break;
					default:
						return {
							content: [{ type: "text", text: `Unknown interaction type: ${params.type}` }],
							details: { error: "unknown-type", type: params.type },
						};
				}
			} catch (error) {
				const message = error instanceof Error ? error.message : String(error);
				return {
					content: [{ type: "text", text: `UI interaction failed: ${message}` }],
					details: { error: "ui-error", message },
				};
			} finally {
				if (isProbing) {
					const event: LifecycleEvent = result?.cancelled ? { type: "waiting-parked" } : { type: "continuing" };
					const transResult = transition(state.lifecycle, event);
					if (transResult.ok) {
						state.lifecycle = transResult.state;
					}
				}
			}

			pi.appendEntry(USER_INPUT_TYPE, {
				interactionType: params.type,
				message: params.message,
				...result.details,
			});

			const toolResult = { content: result.content, details: result.details };
			if (result.cancelled) {
				if (!isProbing) {
					const waitResult = transition(state.lifecycle, { type: "waiting-parked" });
					if (waitResult.ok) {
						state.lifecycle = waitResult.state;
					}
				}
				const activeCommand = getActiveCommand(state.lifecycle);
				if (activeCommand) recordCommandStatus(pi, activeCommand, "waiting_for_user");
				return { ...toolResult, terminate: true };
			}

			return toolResult;
		},
	});
}

async function handleConfirm(message: string, ctx: ExtensionContext) {
	const result = await ctx.ui.custom<"yes" | "no" | null>((tui, theme, _keybindings, done) => {
		const items = [
			{ value: "yes", label: "Yes" },
			{ value: "no", label: "No" },
		];
		const selectList = new MultiLineSelectList(items, 2, {
			selectedText: (text) => theme.fg("accent", text),
			description: (text) => theme.fg("muted", text),
			scrollInfo: (text) => theme.fg("dim", text),
		});
		selectList.onSelect = (item) => done(item.value as "yes" | "no");
		selectList.onCancel = () => done(null);

		return {
			render(width: number) {
				return [
					...wrapTextWithAnsi(theme.fg("accent", theme.bold(message)), width),
					...selectList.render(width),
					theme.fg("dim", "enter select \u2022 esc cancel"),
				];
			},
			invalidate() {
				selectList.invalidate();
			},
			handleInput(data: string) {
				selectList.handleInput(data);
				tui.requestRender();
			},
			dispose() {},
		};
	});

	if (result === null) {
		return {
			content: [{ type: "text" as const, text: JSON.stringify({ cancelled: true }) }],
			details: { type: "confirm", cancelled: true },
			cancelled: true,
		};
	}
	return {
		content: [{ type: "text" as const, text: JSON.stringify({ confirmed: result === "yes" }) }],
		details: { type: "confirm", confirmed: result === "yes" },
		cancelled: false,
	};
}

async function handleSelect(
	message: string,
	options: UserInputOption[],
	recommended: number | undefined,
	ctx: ExtensionContext,
) {
	const items = options.map((opt) => ({
		value: opt.value,
		label: opt.label,
		description: opt.description,
	}));

	const selectedValue = await ctx.ui.custom<string | null>((tui, theme, _keybindings, done) => {
		const selectList = new MultiLineSelectList(
			items,
			Math.min(items.length, 8),
			{
				selectedText: (text) => theme.fg("accent", text),
				description: (text) => theme.fg("muted", text),
				scrollInfo: (text) => theme.fg("dim", text),
			},
			{ recommendedIndex: recommended },
		);

		if (recommended !== undefined) selectList.setSelectedIndex(recommended);

		selectList.onSelect = (item) => done(item.value);
		selectList.onCancel = () => done(null);

		return {
			render(width: number) {
				return [
					...wrapTextWithAnsi(theme.fg("accent", theme.bold(message)), width),
					...selectList.render(width),
					theme.fg("dim", "\u2191\u2193 navigate \u2022 enter select \u2022 esc cancel"),
				];
			},
			invalidate() {
				selectList.invalidate();
			},
			handleInput(data: string) {
				selectList.handleInput(data);
				tui.requestRender();
			},
			dispose() {},
		};
	});

	if (selectedValue === null) {
		return {
			content: [{ type: "text" as const, text: JSON.stringify({ cancelled: true }) }],
			details: { type: "select", cancelled: true },
			cancelled: true,
		};
	}
	return {
		content: [{ type: "text" as const, text: JSON.stringify({ selected: selectedValue }) }],
		details: { type: "select", selected: selectedValue },
		cancelled: false,
	};
}

function validateParams(params: UserInputParams): string | null {
	if (!params.message || params.message.trim() === "") {
		return "Validation error: 'message' is required and must be non-empty.";
	}

	if (params.type === "select") {
		if (!Array.isArray(params.options) || params.options.length === 0) {
			return "Validation error: 'options' is required and must be a non-empty array for select type.";
		}
		const invalidOptionIndex = params.options.findIndex(
			(option) =>
				typeof option !== "object" ||
				option === null ||
				typeof option.value !== "string" ||
				option.value.trim() === "" ||
				typeof option.label !== "string" ||
				option.label.trim() === "",
		);
		if (invalidOptionIndex !== -1) {
			return `Validation error: 'options[${invalidOptionIndex}]' must include non-empty string 'value' and 'label'.`;
		}
		if (params.recommended !== undefined && (params.recommended < 0 || params.recommended >= params.options.length)) {
			return `Validation error: 'recommended' index ${params.recommended} is out of range (0-${params.options.length - 1}).`;
		}
	}

	return null;
}
