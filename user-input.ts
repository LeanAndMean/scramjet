import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { type Static, Type } from "typebox";
import { MultiLineSelectList } from "./multi-line-select.ts";
import type { ScramjetState } from "./types.ts";

export const USER_INPUT_TYPE = "scramjet:user-input";

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
		name: "scramjet_user_input",
		label: "Scramjet User Input",
		description:
			"Request structured input from the user during command execution. " +
			"Supports confirm (yes/no), select (pick from options), and freetext (open-ended input). " +
			"Blocks until the user responds; the turn continues after the response.",
		promptSnippet: PROMPT_SNIPPET,
		parameters: USER_INPUT_SCHEMA,
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

			const isProbing = state.commandPhase === "probing";
			if (isProbing) state.suspendProbeWatchdog?.();

			type InteractionResult = {
				content: { type: "text"; text: string }[];
				details: Record<string, unknown>;
			};
			let result: InteractionResult;
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
					case "freetext":
						result = await handleFreetext(params.message, params.placeholder, ctx as ExtensionContext);
						break;
				}
			} catch (error) {
				const message = error instanceof Error ? error.message : String(error);
				return {
					content: [{ type: "text", text: `UI interaction failed: ${message}` }],
					details: { error: "ui-error", message },
				};
			} finally {
				if (isProbing) state.rearmProbeWatchdog?.();
			}

			pi.appendEntry(USER_INPUT_TYPE, {
				interactionType: params.type,
				message: params.message,
				...result.details,
			});

			return result;
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
					theme.fg("accent", theme.bold(message)),
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
		};
	}
	return {
		content: [{ type: "text" as const, text: JSON.stringify({ confirmed: result === "yes" }) }],
		details: { type: "confirm", confirmed: result === "yes" },
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
					theme.fg("accent", theme.bold(message)),
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
		};
	}
	return {
		content: [{ type: "text" as const, text: JSON.stringify({ selected: selectedValue }) }],
		details: { type: "select", selected: selectedValue },
	};
}

async function handleFreetext(message: string, placeholder: string | undefined, ctx: ExtensionContext) {
	const text = await ctx.ui.input(message, placeholder);
	if (text === undefined) {
		return {
			content: [{ type: "text" as const, text: JSON.stringify({ cancelled: true }) }],
			details: { type: "freetext", cancelled: true },
		};
	}
	return { content: [{ type: "text" as const, text: JSON.stringify({ text }) }], details: { type: "freetext", text } };
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
