/**
 * scramjet_next_step_selection harness-only record tool (issue 324).
 *
 * The next-step selector is a transient ctx.ui.custom widget — it vanishes on
 * resolution and leaves no transcript artifact. This tool, invoked by
 * auto-continue.ts via pi.invokeHarnessTool after the selector resolves (or
 * after a headless routeWithoutUi auto-dispatch), records the outcome as a real
 * ToolResultMessage: persisted, replayed, rendered as a TUI row, and visible in
 * LLM context so the model learns which next step was actually taken. Modeled
 * on model-change-notice.ts.
 */

import type { AgentToolResult, ExtensionAPI } from "@leanandmean/coding-agent";
import { Container, Text } from "@leanandmean/tui";
import { type Static, Type } from "typebox";

export const NEXT_STEP_RECORD_TOOL = "scramjet_next_step_selection";

const RECORD_OPTION_SCHEMA = Type.Object({
	message: Type.String(),
	reason: Type.Optional(Type.String()),
});

export const NEXT_STEP_RECORD_SCHEMA = Type.Object({
	outcome: Type.Union([Type.Literal("selected"), Type.Literal("dismissed")]),
	options: Type.Array(RECORD_OPTION_SCHEMA),
	selected: Type.Union([Type.String(), Type.Null()], {
		description: "Message of the selected option, or null when dismissed.",
	}),
	sourceCommand: Type.Union([Type.String(), Type.Null()]),
	source: Type.Union([Type.Literal("completion"), Type.Literal("suggestion")]),
});

export type NextStepRecordParams = Static<typeof NEXT_STEP_RECORD_SCHEMA>;

function heading(params: NextStepRecordParams): string {
	if (params.source === "suggestion") return "Agent-suggested next step:";
	return params.sourceCommand ? `Next step (from ${params.sourceCommand}):` : "Next step:";
}

function optionLine(option: Static<typeof RECORD_OPTION_SCHEMA>, prefix: string): string {
	const reason = option.reason ? ` — ${option.reason}` : "";
	return `${prefix}${option.message}${reason}`;
}

export function buildRecordText(params: NextStepRecordParams): string {
	const lines: string[] = [heading(params)];
	if (params.outcome === "selected") {
		for (const option of params.options) {
			lines.push(optionLine(option, option.message === params.selected ? "→ " : "  "));
		}
	} else {
		for (const option of params.options) {
			lines.push(optionLine(option, "- "));
		}
		lines.push("Cancelled");
	}
	return lines.join("\n");
}

function isRecordParams(value: unknown): value is NextStepRecordParams {
	if (typeof value !== "object" || value === null) return false;
	const record = value as Partial<NextStepRecordParams>;
	return (
		(record.outcome === "selected" || record.outcome === "dismissed") &&
		Array.isArray(record.options) &&
		(record.selected === null || typeof record.selected === "string")
	);
}

function renderRecordResult(result: AgentToolResult<unknown>) {
	if (!isRecordParams(result.details)) return new Text("", 0, 0);
	const container = new Container();
	container.addChild(new Text(buildRecordText(result.details), 0, 0));
	return container;
}

export function registerNextStepRecord(pi: ExtensionAPI): void {
	// Structurally harness-only: the harness invokes it after next-step selector
	// resolution; the model can never call it (never in the provider-visible set).
	pi.registerTool({
		name: NEXT_STEP_RECORD_TOOL,
		label: "Next Step Selection",
		description:
			"System-generated record of a next-step selector outcome. Invoked by the harness (never by " +
			"the model) to persist which next-step options were offered and which one the user selected " +
			"or whether the offer was dismissed.",
		activation: "harness-only",
		parameters: NEXT_STEP_RECORD_SCHEMA,
		renderResult(result) {
			return renderRecordResult(result);
		},
		async execute(_toolCallId, params) {
			return {
				content: [{ type: "text", text: buildRecordText(params) }],
				details: params,
			};
		},
	});
}
