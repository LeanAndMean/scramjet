/**
 * report_scramjet_command_status tool: the agent's structured report at the
 * end of an active Scramjet command, supplied in a *separate* turn from the
 * command's user-facing answer (issue 84, two-phase protocol).
 *
 * The command's normal answer turn injects nothing about completion. After that
 * turn goes idle, auto-continue.ts defers a hidden status-check probe (see
 * buildProbeMessage); the agent answers it by calling this tool. execute() is
 * phase-gated — it only accepts the report while commandPhase === "probing".
 *
 * Five statuses, two execution paths:
 * - "continuing": non-terminating. The agent has more work to do; the tool
 *   transitions probing → running and returns without terminate so the agent
 *   keeps working in the same turn. A local counter bounds consecutive
 *   continues (MAX_CONSECUTIVE_CONTINUES) to prevent infinite loops.
 * - "completed" / "waiting_for_user" / "blocked" / "incomplete": terminating.
 *   The tool stores the report on ScramjetState, advances the phase to
 *   "reported", and returns terminate: true. auto-continue.ts reads the stored
 *   status on the probe turn's agent_end and validates/dispatches/pauses.
 *
 * This replaces the old generic `task_complete` tool, whose same-turn,
 * summary-bearing shape invited the model to pour its answer into the tool
 * payload instead of writing prose. There is no completion tool during the
 * answer turn anymore, so that failure mode is removed structurally.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { type Static, Type } from "typebox";
import { parseSlashCommand } from "./commands/validator.ts";
import { recordCommandStatus } from "./history.ts";
import { transitionPhase } from "./phase-machine.ts";
import type { CommandPhase, CommandStatusNextStep, CommandStatusPayload, ScramjetState } from "./types.ts";

// Shared shape for the tool's result `details` so both the out-of-phase error
// branch and the success branch infer the same TDetails (mirrors delegate.ts's
// DelegateDetails pattern).
interface CommandStatusDetails {
	error?: string;
	phase?: CommandPhase;
	status?: CommandStatusPayload["status"];
	summary?: string;
	recommended_next_step?: number;
	message?: string;
	fresh_session?: boolean;
	reason?: string;
}

// customType for the hidden status-check probe message. display:false keeps it
// out of the TUI; it still persists in the journal and reaches the model as a
// user-role message that asks for exactly one report_scramjet_command_status call.
export const COMMAND_STATUS_PROBE_TYPE = "scramjet-command-status";

const MAX_CONSECUTIVE_CONTINUES = 3;

const OUT_OF_PHASE_ERROR =
	"report_scramjet_command_status is not active right now. Do not call this tool for ordinary tasks — " +
	"call it only when Scramjet's status-check message explicitly asks you to report command status.";

const CONTINUE_LIMIT_ERROR =
	`You have reported "continuing" ${MAX_CONSECUTIVE_CONTINUES} times without completing the command. ` +
	"Report your actual status: completed, blocked, waiting_for_user, or incomplete.";

// F6: single source of truth for the next-step wire shape. The TypeBox schema
// below and the CommandStatusNextStep TS interface (types.ts) are two
// declarations of the same payload. The congruence guards underneath fail the
// build if either side renames, adds, or drops a field — so a `fresh_session`
// rename can't typecheck clean while silently breaking the runtime contract.
const NEXT_STEP_SCHEMA = Type.Object({
	message: Type.String({
		description:
			"The suggested next message, shown to the user verbatim and dispatched on selection. " +
			"For a slash command, start with '/' and include any arguments, e.g. '/mach12:issue-plan 55'. " +
			"For a non-command follow-up, write the message text directly.",
	}),
	fresh_session: Type.Optional(
		Type.Boolean({
			description:
				"Whether to start a fresh session first (true if instructions say '/clear then ...' or 'in a fresh session'). " +
				"Only meaningful for slash commands; defaults to false.",
		}),
	),
	reason: Type.Optional(Type.String({ description: "Brief explanation of why this next step fits." })),
});

// Bidirectional assignability: each direction fails to compile if the schema and
// the interface diverge (a rename drops a required field from one side's view).
type WireNextStep = Static<typeof NEXT_STEP_SCHEMA>;
const _wireMatchesInterface = (step: WireNextStep): CommandStatusNextStep => step;
const _interfaceMatchesWire = (step: CommandStatusNextStep): WireNextStep => step;

// F3: single source of truth for the status enum, mirroring the next_steps
// congruence guards above. The TypeBox union below and the
// CommandStatusPayload["status"] TS union (types.ts) are two declarations of the
// same five literals; the assignability pair underneath fails the build if
// either side adds, drops, or renames a status (e.g. adding "cancelled" to one
// side only), closing the last drift hole the rest of this file already guards.
const STATUS_SCHEMA = Type.Union(
	[
		Type.Literal("completed"),
		Type.Literal("waiting_for_user"),
		Type.Literal("blocked"),
		Type.Literal("incomplete"),
		Type.Literal("continuing"),
	],
	{
		description:
			"completed = the command's work is done and your final user-facing answer was already delivered; " +
			"continuing = you have more work to do right now (not blocked, not waiting for input, not finished); " +
			"waiting_for_user = you asked the user a question or need input before continuing; " +
			"blocked = the command cannot proceed (error, missing dependency, authorization); " +
			"incomplete = none of the above (stopped without a clean completion/question/blocker).",
	},
);

type WireStatus = Static<typeof STATUS_SCHEMA>;
const _statusWireMatchesInterface = (status: WireStatus): CommandStatusPayload["status"] => status;
const _statusInterfaceMatchesWire = (status: CommandStatusPayload["status"]): WireStatus => status;

export function registerCommandStatusTool(pi: ExtensionAPI, state: ScramjetState) {
	// Accumulates across probe cycles (each probe is a new turn, so a
	// before_agent_start reset would clear the counter every cycle and make
	// the limit unreachable). Reset only on the terminating path below.
	let consecutiveContinues = 0;
	state.resetConsecutiveContinues = () => {
		consecutiveContinues = 0;
	};

	pi.registerTool({
		name: "report_scramjet_command_status",
		label: "Report Scramjet Command Status",
		description:
			"Report the status of an active Scramjet slash command after Scramjet explicitly asks for a status check. " +
			"Do not call this tool for ordinary user tasks. Do not call it unless the latest message asks you to call it.",
		parameters: Type.Object({
			status: STATUS_SCHEMA,
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
						"Each entry is a suggested next message; entries may reuse the same slash command with " +
						"different arguments to offer meaningful variants.",
				}),
			),
			recommended_next_step: Type.Optional(
				Type.Integer({
					minimum: 0,
					description: "Zero-based index into next_steps for the recommended selector option.",
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
				console.warn(
					`scramjet: report_scramjet_command_status called out of phase (phase=${state.commandPhase}); report ignored`,
				);
				const details: CommandStatusDetails = { error: "out-of-phase", phase: state.commandPhase };
				return {
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
					details,
				};
			}

			// Non-terminating path: the agent has more work to do.
			if (params.status === "continuing") {
				consecutiveContinues++;
				if (consecutiveContinues > MAX_CONSECUTIVE_CONTINUES) {
					const details: CommandStatusDetails = { error: "continue-limit", phase: state.commandPhase };
					return {
						content: [{ type: "text", text: CONTINUE_LIMIT_ERROR }],
						details,
					};
				}
				state.suspendProbeWatchdog?.();
				transitionPhase(state, "running");
				state.rearmProbeWatchdog?.();
				const details: CommandStatusDetails = { status: "continuing", summary: params.summary };
				return {
					content: [{ type: "text", text: "Continuing. Proceed with your work." }],
					details,
				};
			}

			// Terminating path: reset the continue counter.
			consecutiveContinues = 0;

			const payload: CommandStatusPayload = {
				status: params.status,
				summary: params.summary,
				user_prompt: params.user_prompt,
				next_steps: params.next_steps,
				recommended_next_step: params.recommended_next_step,
			};
			state.latestCommandStatus = payload;
			if (!transitionPhase(state, "reported")) {
				state.latestCommandStatus = null;
				const details: CommandStatusDetails = { error: "phase-transition-failed", phase: state.commandPhase };
				return {
					content: [{ type: "text", text: "Status recorded but phase transition to reported failed." }],
					details,
					terminate: true,
				};
			}

			// issue 88: journal the report so a rewind/resume can reconstruct the
			// resumable "waiting" state — and, just as importantly, reconstruct a
			// command that completed-without-chaining to "idle" rather than
			// resurrecting it. Guarded on a non-null active command: the phase gate
			// above means we're mid-probe for an active invocation, but the field is
			// typed nullable, and journaling an entry with no command name would be
			// skipped by replayHistory's defensive filter anyway.
			if (state.activeTopLevelCommand) {
				recordCommandStatus(pi, state.activeTopLevelCommand, params.status);
			}

			const next =
				params.recommended_next_step === undefined
					? params.next_steps?.[0]
					: params.next_steps?.[params.recommended_next_step];
			// Forward pointer only for slash-command messages: a non-command
			// message is pasted, not dispatched, so an arrow would overstate it.
			const text =
				params.status === "completed" && next && parseSlashCommand(next.message)
					? `→ ${next.message.trim()}`
					: `status: ${params.status}`;

			const details: CommandStatusDetails = {
				status: params.status,
				summary: params.summary,
				recommended_next_step: params.recommended_next_step,
				...(next ?? {}),
			};
			return {
				content: [{ type: "text", text }],
				details,
				terminate: true,
			};
		},
	});
}
