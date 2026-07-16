/**
 * scramjet_next_step_selection harness-only record tool (issue 324).
 *
 * The next-step selector is a transient ctx.ui.custom widget — it vanishes on
 * resolution and leaves no transcript artifact. This tool, invoked by
 * auto-continue.ts via pi.invokeHarnessTool after the selector resolves (or,
 * on the headless routeWithoutUi path, before the auto-dispatch fires),
 * records the outcome as a real
 * ToolResultMessage: persisted, replayed, rendered as a TUI row, and visible in
 * LLM context so the model can see which next-step option the selector or harness
 * selected (or that the offer was dismissed). A recorded selection reflects the
 * selector/harness outcome, not proof the step was dispatched — a post-record
 * staleness guard can still suppress dispatch. Modeled on model-change-notice.ts.
 */

import type { AgentToolResult, ExtensionAPI } from "@leanandmean/coding-agent";
import { Text } from "@leanandmean/tui";
import { type Static, Type } from "typebox";

export const NEXT_STEP_RECORD_TOOL = "scramjet_next_step_selection";

const RECORD_OPTION_SCHEMA = Type.Object({
	message: Type.String(),
	reason: Type.Optional(Type.String()),
});

export const NEXT_STEP_RECORD_SCHEMA = Type.Object({
	outcome: Type.Union([Type.Literal("selected"), Type.Literal("dismissed")]),
	options: Type.Array(RECORD_OPTION_SCHEMA),
	selectedIndex: Type.Union([Type.Integer(), Type.Null()], {
		description: "Index into options of the selected option, or null when dismissed.",
	}),
	sourceCommand: Type.Union([Type.String(), Type.Null()]),
	source: Type.Union([Type.Literal("completion"), Type.Literal("suggestion")]),
});

export type NextStepRecordParams = Static<typeof NEXT_STEP_RECORD_SCHEMA>;

// Option message/reason and the source command are model-authored. The transient
// selector strips C0/DEL controls and collapses whitespace before display
// (next-step-selector.ts cleanDisplay); this persistent, replayed row must do the
// same so a stored ESC/BEL/newline can't spoof the record layout on every resume.
function cleanDisplay(text: string): string {
	return text
		.replace(/[\u0000-\u001f\u007f]/g, " ")
		.trim()
		.replace(/\s+/g, " ");
}

function heading(params: NextStepRecordParams): string {
	if (params.source === "suggestion") return "Agent-suggested next step:";
	return params.sourceCommand ? `Next step (from ${cleanDisplay(params.sourceCommand)}):` : "Next step:";
}

function optionLine(option: Static<typeof RECORD_OPTION_SCHEMA>, prefix: string): string {
	const reason = option.reason ? cleanDisplay(option.reason) : "";
	const suffix = reason ? ` — ${reason}` : "";
	return `${prefix}${cleanDisplay(option.message)}${suffix}`;
}

export function buildRecordText(params: NextStepRecordParams): string {
	const lines: string[] = [heading(params)];
	if (params.outcome === "selected") {
		params.options.forEach((option, index) => {
			lines.push(optionLine(option, index === params.selectedIndex ? "→ " : "  "));
		});
	} else {
		for (const option of params.options) {
			lines.push(optionLine(option, "- "));
		}
		lines.push("Cancelled");
	}
	return lines.join("\n");
}

function renderRecordResult(result: AgentToolResult<unknown>) {
	const text = result.content.find((c) => c.type === "text")?.text;
	return new Text(text ?? "(next-step record unavailable)", 0, 0);
}

export function registerNextStepRecord(pi: ExtensionAPI): void {
	// Structurally harness-only: the harness invokes it after next-step selector
	// resolution; the model can never call it (never in the provider-visible set).
	pi.registerTool({
		name: NEXT_STEP_RECORD_TOOL,
		label: "Next Step Selection",
		description:
			"System-generated record of a next-step selector outcome. Invoked by the harness (never by " +
			"the model) to persist which next-step options were offered and which one was selected — by " +
			"the user in the selector or automatically under headless autopilot — or whether the offer " +
			"was dismissed.",
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
