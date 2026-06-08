/**
 * scramjet_command_status tool: the agent's structured report at the end of an
 * active Scramjet command, supplied in a *separate* turn from the command's
 * user-facing answer (issue 84, two-phase protocol).
 *
 * The command's normal answer turn injects nothing about completion. After that
 * turn goes idle, auto-continue.ts defers a hidden status-check probe (see
 * buildProbeMessage); the agent answers it by calling this tool. execute() is
 * phase-gated — it only accepts the report while commandPhase === "probing",
 * stores it on ScramjetState, advances the phase to "reported", and terminates
 * the short probe turn. auto-continue.ts reads the stored status on the probe
 * turn's agent_end and validates/dispatches/pauses.
 *
 * This replaces the old generic `task_complete` tool, whose same-turn,
 * summary-bearing shape invited the model to pour its answer into the tool
 * payload instead of writing prose. There is no completion tool during the
 * answer turn anymore, so that failure mode is removed structurally.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { type Static, Type } from "typebox";
import type { CommandPhase, CommandStatusNextStep, CommandStatusPayload, ScramjetState } from "./types.ts";

// Shared shape for the tool's result `details` so both the out-of-phase error
// branch and the success branch infer the same TDetails (mirrors delegate.ts's
// DelegateDetails pattern).
interface CommandStatusDetails extends Partial<CommandStatusNextStep> {
	error?: string;
	phase?: CommandPhase;
	status?: CommandStatusPayload["status"];
	summary?: string;
}

// customType for the hidden status-check probe message. display:false keeps it
// out of the TUI; it still persists in the journal and reaches the model as a
// user-role message that asks for exactly one scramjet_command_status call.
export const COMMAND_STATUS_PROBE_TYPE = "scramjet-command-status";

// Error text returned (without terminate) when the tool is called outside the
// probe phase. Phrased to teach the model the harness-enforced contract: the
// tool is only valid in direct response to the injected status-check message.
const OUT_OF_PHASE_ERROR =
	"scramjet_command_status is not active right now. Do not call this tool for ordinary tasks — " +
	"call it only when Scramjet's status-check message explicitly asks you to report command status.";

// F6: single source of truth for the next-step wire shape. The TypeBox schema
// below and the CommandStatusNextStep TS interface (types.ts) are two
// declarations of the same payload. The congruence guards underneath fail the
// build if either side renames, adds, or drops a field — so a `fresh_session`
// rename can't typecheck clean while silently breaking the runtime contract.
const NEXT_STEP_SCHEMA = Type.Object({
	name: Type.String({
		description:
			"Bare command name (no leading slash, no arguments), e.g. 'mach12:issue-plan'. Must match the declared target for forced policies and one of the listed candidates for closed policies.",
	}),
	args: Type.Optional(
		Type.String({
			description:
				"Optional argument string passed to the command verbatim (no leading space), e.g. '55' or '36 --review-comment 12345'.",
		}),
	),
	fresh_session: Type.Boolean({
		description:
			"Whether to start a fresh session first (true if instructions say '/clear then ...' or 'in a fresh session').",
	}),
	label: Type.Optional(Type.String({ description: "Optional short label for a future choice-list UI." })),
	reason: Type.Optional(Type.String({ description: "Brief explanation of why this next step fits." })),
});

// Bidirectional assignability: each direction fails to compile if the schema and
// the interface diverge (a rename drops a required field from one side's view).
type WireNextStep = Static<typeof NEXT_STEP_SCHEMA>;
const _wireMatchesInterface = (step: WireNextStep): CommandStatusNextStep => step;
const _interfaceMatchesWire = (step: CommandStatusNextStep): WireNextStep => step;

export function registerCommandStatusTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "scramjet_command_status",
		label: "Scramjet Command Status",
		description:
			"Report the status of an active Scramjet slash command after Scramjet explicitly asks for a status check. " +
			"Do not call this tool for ordinary user tasks. Do not call it unless the latest message asks you to call it.",
		parameters: Type.Object({
			status: Type.Union(
				[
					Type.Literal("completed"),
					Type.Literal("waiting_for_user"),
					Type.Literal("blocked"),
					Type.Literal("incomplete"),
				],
				{
					description:
						"completed = the command's work is done and your final user-facing answer was already delivered; " +
						"waiting_for_user = you asked the user a question or need input before continuing; " +
						"blocked = the command cannot proceed (error, missing dependency, authorization); " +
						"incomplete = none of the above (stopped without a clean completion/question/blocker).",
				},
			),
			summary: Type.String({ description: "Brief summary of the command's outcome." }),
			user_prompt: Type.Optional(
				Type.String({
					description: "For waiting_for_user: the question or input you are waiting on, if useful to surface.",
				}),
			),
			next_steps: Type.Optional(
				Type.Array(NEXT_STEP_SCHEMA, {
					description:
						"Ordered next-step candidates for completed commands. Omit entirely to stop the chain. " +
						"The first entry valid for the command's policy is acted on; the array shape carries candidates for a future choice-list UI.",
				}),
			),
		}),
		async execute(_toolCallId, params) {
			// Harness-enforced phase gate (issue 84 anti-pattern #3): prose alone
			// cannot keep the model from calling this tool whenever it finishes a
			// task. The tool is only meaningful in direct response to the injected
			// status-check probe, i.e. while commandPhase === "probing". Outside
			// that window, return a helpful error WITHOUT terminate so the model's
			// real turn is not cut short.
			if (state.commandPhase !== "probing") {
				const details: CommandStatusDetails = { error: "out-of-phase", phase: state.commandPhase };
				return {
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
					details,
				};
			}

			const payload: CommandStatusPayload = {
				status: params.status,
				summary: params.summary,
				user_prompt: params.user_prompt,
				next_steps: params.next_steps,
			};
			state.latestCommandStatus = payload;
			state.commandPhase = "reported";

			const next = params.next_steps?.[0];
			const text =
				params.status === "completed" && next
					? `→ /${next.name}${next.args ? ` ${next.args}` : ""}`
					: `status: ${params.status}`;

			const details: CommandStatusDetails = { status: params.status, summary: params.summary, ...(next ?? {}) };
			return {
				content: [{ type: "text", text }],
				details,
				terminate: true,
			};
		},
	});
}
