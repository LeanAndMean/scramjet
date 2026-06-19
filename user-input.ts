import type { AgentToolResult, ExtensionAPI, ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
import { Text, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { type Static, Type } from "typebox";
import { USER_INPUT_PARKED_TYPE } from "./history.ts";
import { MultiLineSelectList } from "./multi-line-select.ts";
import { getActiveCommand, type LifecycleEvent, logTransition, transition } from "./phase-machine.ts";
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
	"Prompt messages remain visible in the tool row/result history; select results also show the available options. " +
	"State surrounding context in prose before calling the tool when the user needs that context to answer.";

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
		Type.String({
			description:
				"For freetext type: placeholder hint text shown in the input. Accepted for compatibility but unused by freetext.",
		}),
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
		renderCall(args, theme, context) {
			const maybeArgs = args as Partial<UserInputParams> | null | undefined;
			const message = typeof maybeArgs?.message === "string" ? maybeArgs.message : "";
			let text = theme.fg("toolTitle", theme.bold("get_scramjet_user_input"));
			if (message) text += ` ${theme.fg("muted", message)}`;
			const component = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
			component.setText(text);
			return component;
		},
		renderResult(result, _options, theme, context) {
			return renderUserInputResult(result, theme, context.args as Partial<UserInputParams> | null | undefined);
		},
		async execute(_toolCallId, params, _resource, _read, ctx) {
			if (!ALLOWED_PHASES.has(state.lifecycle.phase)) {
				state.logger.warn(
					"input",
					`get_scramjet_user_input called out of phase (phase=${state.lifecycle.phase}); rejected`,
					{
						phase: state.lifecycle.phase,
					},
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
				const from = state.lifecycle;
				const waitResult = transition(state.lifecycle, { type: "waiting-parked" });
				if (waitResult.ok) {
					state.lifecycle = waitResult.state;
					logTransition(state, from, waitResult.state, "waiting-parked", { inputType: params.type });
				}
				const activeCommand = getActiveCommand(state.lifecycle);
				if (activeCommand) pi.appendEntry(USER_INPUT_PARKED_TYPE, { commandName: activeCommand });
				return {
					content: [{ type: "text", text: JSON.stringify({ parked: true }) }],
					details: { type: "freetext", parked: true },
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
				if (isProbing) state.rearmProbeWatchdog?.();
				const message = error instanceof Error ? error.message : String(error);
				return {
					content: [{ type: "text", text: `UI interaction failed: ${message}` }],
					details: { error: "ui-error", message },
				};
			} finally {
				if (isProbing && result) {
					const event: LifecycleEvent = result.cancelled
						? { type: "waiting-parked" }
						: { type: "probe-input-resumed" };
					const from = state.lifecycle;
					const transResult = transition(state.lifecycle, event);
					if (transResult.ok) {
						state.lifecycle = transResult.state;
						logTransition(state, from, transResult.state, event.type, {
							inputType: params.type,
							cancelled: result.cancelled,
						});
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
					const from = state.lifecycle;
					const waitResult = transition(state.lifecycle, { type: "waiting-parked" });
					if (waitResult.ok) {
						state.lifecycle = waitResult.state;
						logTransition(state, from, waitResult.state, "waiting-parked", {
							inputType: params.type,
							cancelled: true,
						});
					}
				}
				const activeCommand = getActiveCommand(state.lifecycle);
				if (activeCommand) pi.appendEntry(USER_INPUT_PARKED_TYPE, { commandName: activeCommand });
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
			details: { type: "select", cancelled: true, options },
			cancelled: true,
		};
	}
	return {
		content: [{ type: "text" as const, text: JSON.stringify({ selected: selectedValue }) }],
		details: { type: "select", selected: selectedValue, options },
		cancelled: false,
	};
}

function renderUserInputResult(
	result: AgentToolResult<unknown>,
	theme: Theme,
	args: Partial<UserInputParams> | null | undefined,
) {
	const details = isRecord(result.details) ? result.details : null;
	if (!details) return new Text("", 0, 0);

	if ("error" in details) {
		const text = result.content.find((item) => item.type === "text")?.text ?? "";
		return new Text(text, 0, 0);
	}

	const message = typeof args?.message === "string" ? args.message : "";
	const messageLine = message ? theme.fg("accent", theme.bold(message)) : "";

	if (details.type === "confirm") {
		if (details.cancelled === true) return new Text(compactLines([messageLine, "Cancelled"]).join("\n"), 0, 0);
		if (typeof details.confirmed === "boolean") {
			return new Text(compactLines([messageLine, `Answer: ${details.confirmed ? "Yes" : "No"}`]).join("\n"), 0, 0);
		}
		return new Text("", 0, 0);
	}

	if (details.type === "select") {
		const options = parseUserInputOptions(details.options);
		if (!options) return new Text("", 0, 0);
		const optionLines = options.map((option) => {
			const description = option.description ? ` — ${option.description}` : "";
			return `- ${option.label}${description}`;
		});
		const outcome =
			details.cancelled === true
				? "Cancelled"
				: typeof details.selected === "string"
					? `Selected: ${details.selected}`
					: "";
		if (!outcome) return new Text("", 0, 0);
		return new Text(compactLines([messageLine, "Options:", ...optionLines, outcome]).join("\n"), 0, 0);
	}

	if (details.type === "freetext") {
		if (details.parked === true) return new Text(compactLines([messageLine, "Parked for reply"]).join("\n"), 0, 0);
		return new Text("", 0, 0);
	}

	return new Text("", 0, 0);
}

function parseUserInputOptions(value: unknown): UserInputOption[] | null {
	if (!Array.isArray(value)) return null;
	for (const option of value) {
		if (
			!isRecord(option) ||
			typeof option.value !== "string" ||
			typeof option.label !== "string" ||
			(option.description !== undefined && typeof option.description !== "string")
		) {
			return null;
		}
	}
	return value as UserInputOption[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function compactLines(lines: string[]) {
	return lines.filter((line) => line.length > 0);
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
