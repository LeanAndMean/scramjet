/**
 * report_scramjet_command_status tool and dormant command notice.
 *
 * The tool is the agent's structured report at the end of an active Scramjet
 * command. Terminal statuses (completed/blocked/incomplete) terminate the turn
 * and are accepted inline once the answer is delivered (issue 331), during a
 * probe, or from dormant; "continuing" is non-terminating and re-arms the
 * probe (from a probe turn or dormant only). auto-continue.ts reads the stored
 * status on the subsequent agent_end and validates/dispatches/pauses.
 *
 * The dormant notice is a volatile system prompt section that tells the agent
 * about a dormant command and how to resume or complete it.
 */

import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { type Static, Type } from "typebox";
import { parseSlashCommand } from "./commands/validator.js";
import { recordCommandStatus } from "./history.js";
import {
	acceptDormantContinuing,
	acceptProbeContinuing,
	acceptTerminalReport,
	activeCommandName,
	CONTINUE_LIMIT,
	canAcceptDormantContinuing,
	canAcceptTerminalReport,
	isDormant,
	isProbeInFlight,
} from "./lifecycle.js";
import type {
	CommandStatusNextStep,
	CommandStatusPayload,
	CommandStatusRestingPayload,
	ScramjetState,
} from "./types.js";

interface CommandStatusDetails {
	error?: string;
	phase?: string;
	status?: CommandStatusPayload["status"];
	summary?: string;
	recommended_next_step?: number;
	message?: string;
	fresh_session?: boolean;
	reason?: string;
}

// customType for the hidden status-check probe message.
export const COMMAND_STATUS_PROBE_TYPE = "scramjet-command-status";

const NO_ACTIVE_COMMAND_ERROR =
	"report_scramjet_command_status is not active right now — no Scramjet command is running. " +
	"Use it only during an active Scramjet slash command: report a terminal status inline once the " +
	"command's work is done and the final answer is delivered, or in response to Scramjet's " +
	"status-check message. Do not call this tool for ordinary tasks.";

const CONTINUE_LIMIT_ERROR =
	`You have reported "continuing" ${CONTINUE_LIMIT} times without completing the command. ` +
	"Report your actual status: completed, blocked, or incomplete.";

const PROMPT_SNIPPET =
	"You have access to `report_scramjet_command_status` for reporting the status of an active " +
	"Scramjet slash command. Summarize the work you performed first, then give the status — the " +
	"summary is your evidence, the status is your assessment of it. Once the command's work is done " +
	"and your final user-facing answer has been delivered, you may report a terminal status " +
	"(`completed`, `blocked`, or `incomplete`) inline — always deliver the complete answer first, " +
	"because reporting ends the turn. If you do not report inline, Scramjet sends a status-check " +
	"message as a fallback; respond to it with this tool. Do not call this tool for ordinary user tasks.";

const PARKED_ERROR =
	"report_scramjet_command_status cannot accept a report right now: the command is parked waiting " +
	"for user input. Wait for the user's reply before reporting.";

const ALREADY_REPORTED_ERROR =
	"report_scramjet_command_status cannot accept a report right now: a status report was already " +
	"filed for this command. Do not call this tool again.";

const CONTINUING_OUT_OF_PHASE_ERROR =
	'report_scramjet_command_status cannot accept "continuing" right now — it is only valid in ' +
	"response to Scramjet's status-check message or while the command is dormant. Keep working, " +
	"or report a terminal status (completed, blocked, incomplete) once the work is done.";

// Reached only when canAcceptTerminalReport is false and the command is active, which by the
// mode-exclusivity invariant leaves exactly two states: reported (lastReport !== null) or parked.
// A fifth mode flag would break this binary discrimination.
function outOfPhaseError(lifecycle: ScramjetState["lifecycle"]): string {
	return lifecycle.lastReport !== null ? ALREADY_REPORTED_ERROR : PARKED_ERROR;
}

// F6: single source of truth for the next-step wire shape.
export const NEXT_STEP_SCHEMA = Type.Object({
	message: Type.String({
		description:
			"The suggested next message, shown to the user verbatim and dispatched on selection. " +
			"For a slash command, start with '/' and include any arguments, e.g. '/mach12:issue-plan 55'. " +
			"For a non-command follow-up, write the message text directly.",
	}),
	fresh_session: Type.Boolean({
		description:
			"Whether to start a fresh session first (true if instructions say '/clear then ...' or 'in a fresh session'). " +
			"Only meaningful for slash commands; defaults to false.",
	}),
	reason: Type.Optional(Type.String({ description: "Brief explanation of why this next step fits." })),
});

type WireNextStep = Static<typeof NEXT_STEP_SCHEMA>;
const _wireMatchesInterface = (step: WireNextStep): CommandStatusNextStep => step;
const _interfaceMatchesWire = (step: CommandStatusNextStep): WireNextStep => step;

// F3: single source of truth for the status enum.
const STATUS_SCHEMA = Type.Union(
	[Type.Literal("completed"), Type.Literal("blocked"), Type.Literal("incomplete"), Type.Literal("continuing")],
	{
		description:
			"completed = the command's work is done and your final user-facing answer was already delivered; " +
			"continuing = you have more work to do right now (not blocked, not waiting for input, not finished); " +
			"blocked = the command cannot proceed (error, missing dependency, authorization); " +
			"incomplete = none of the above (stopped without a clean completion/question/blocker).",
	},
);

type WireStatus = Static<typeof STATUS_SCHEMA>;
const _statusWireMatchesInterface = (status: WireStatus): CommandStatusPayload["status"] => status;
const _statusInterfaceMatchesWire = (status: CommandStatusPayload["status"]): WireStatus => status;

export function buildDormantCommandNotice(commandName: string): string {
	return (
		`The command \`${commandName}\` is dormant — it started but is not currently active.\n` +
		"Ordinary user replies do NOT auto-resume a dormant command.\n" +
		"You have two options:\n" +
		'- To resume work, call `report_scramjet_command_status` with `status: "continuing"`.\n' +
		"- If the work is already done, report a terminal status directly " +
		'(`status: "completed"`, `"blocked"`, or `"incomplete"`).\n' +
		"Both paths are accepted from dormant state."
	);
}

export function registerDormantCommandNotice(pi: ExtensionAPI, state: ScramjetState) {
	pi.on("before_agent_start", async () => {
		if (!isDormant(state.lifecycle)) return;
		const command = state.lifecycle.activeCommand!;
		return {
			systemPromptSection: {
				id: "scramjet:dormant-command",
				text: `\n\n# Dormant Scramjet Command\n\n${buildDormantCommandNotice(command)}`,
				cacheRetention: "none",
			},
		};
	});
}

export function registerCommandStatusTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "report_scramjet_command_status",
		label: "Report Scramjet Command Status",
		description:
			"Report the status of an active Scramjet slash command. Summarize the work performed first, then " +
			"give the status. Terminal statuses (completed, blocked, incomplete) may be reported inline once the " +
			"command's work is done and the final user-facing answer has been delivered, or in response to " +
			"Scramjet's status-check message. Do not call this tool for ordinary user tasks.",
		promptSnippet: PROMPT_SNIPPET,
		parameters: Type.Object({
			summary: Type.String({
				minLength: 1,
				pattern: "\\S",
				description:
					"A summary of the work you completed. On your first report, summarize the work done so far; " +
					"on each later report, summarize only the work completed since your previous report.",
			}),
			status: STATUS_SCHEMA,
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
			const command = activeCommandName(state.lifecycle);

			// Gate: no active command
			if (!command) {
				state.logger.lifecycle("status report rejected", {
					phase: "idle",
					detail: { reason: "no-active-command", status: params.status },
				});
				state.logger.warn(
					"status",
					"report_scramjet_command_status called with no active command; report ignored",
					{},
				);
				const details: CommandStatusDetails = { error: "out-of-phase", phase: "idle" };
				return {
					content: [{ type: "text", text: NO_ACTIVE_COMMAND_ERROR }],
					details,
				};
			}

			// Continuing path — two valid contexts: probe in flight, or dormant
			if (params.status === "continuing") {
				// Probe continuing
				if (isProbeInFlight(state.lifecycle)) {
					if (state.lifecycle.continueCount >= CONTINUE_LIMIT) {
						state.logger.lifecycle("status report rejected", {
							command,
							detail: { reason: "continue-limit", continueCount: state.lifecycle.continueCount },
						});
						const details: CommandStatusDetails = { error: "continue-limit" };
						return {
							content: [{ type: "text", text: CONTINUE_LIMIT_ERROR }],
							details,
						};
					}
					state.suspendProbeWatchdog?.();
					const result = acceptProbeContinuing(state);
					if (!result.ok) {
						state.logger.lifecycle("status report rejected", {
							command,
							detail: { reason: "mutation-failed", error: result.reason },
						});
						const details: CommandStatusDetails = { error: "mutation-failed" };
						return {
							content: [{ type: "text", text: "Continuing transition failed." }],
							details,
						};
					}
					state.rearmProbeWatchdog?.();
					recordCommandStatus(pi, command, "continuing", params.summary);
					const details: CommandStatusDetails = { status: "continuing", summary: params.summary };
					return {
						content: [{ type: "text", text: "Continuing. Proceed with your work." }],
						details,
					};
				}

				// Dormant continuing
				if (canAcceptDormantContinuing(state.lifecycle)) {
					const result = acceptDormantContinuing(state);
					if (!result.ok) {
						state.logger.lifecycle("status report rejected", {
							command,
							detail: { reason: "mutation-failed", error: result.reason },
						});
						const details: CommandStatusDetails = { error: "mutation-failed" };
						return {
							content: [{ type: "text", text: "Dormant continuing transition failed." }],
							details,
						};
					}
					state.logger.lifecycle("dormant continuing accepted", {
						command,
						detail: { summary: params.summary },
					});
					recordCommandStatus(pi, command, "continuing", params.summary);
					const details: CommandStatusDetails = { status: "continuing", summary: params.summary };
					return {
						content: [{ type: "text", text: "Continuing. Proceed with your work." }],
						details,
					};
				}

				// Continuing from any other state is rejected
				state.logger.lifecycle("status report rejected", {
					command,
					detail: { reason: "out-of-phase", status: params.status },
				});
				state.logger.warn(
					"status",
					"report_scramjet_command_status continuing called out of phase; report ignored",
					{},
				);
				const details: CommandStatusDetails = { error: "out-of-phase" };
				return {
					content: [{ type: "text", text: CONTINUING_OUT_OF_PHASE_ERROR }],
					details,
				};
			}

			// Terminal status path — accepted while running (inline), during probe, or from dormant
			if (!canAcceptTerminalReport(state.lifecycle)) {
				state.logger.lifecycle("status report rejected", {
					command,
					detail: { reason: "out-of-phase", status: params.status },
				});
				state.logger.warn("status", "report_scramjet_command_status called out of phase; report ignored", {});
				const details: CommandStatusDetails = { error: "out-of-phase" };
				return {
					content: [{ type: "text", text: outOfPhaseError(state.lifecycle) }],
					details,
				};
			}

			const payload: CommandStatusRestingPayload = {
				status: params.status,
				summary: params.summary,
				next_steps: params.next_steps,
				recommended_next_step: params.recommended_next_step,
			};

			const reportResult = acceptTerminalReport(state, payload);
			if (!reportResult.ok) {
				state.logger.lifecycle("status report rejected", {
					command,
					detail: { reason: "mutation-failed", error: reportResult.reason },
				});
				const details: CommandStatusDetails = { error: "mutation-failed" };
				return {
					content: [{ type: "text", text: `Status report failed: ${reportResult.reason}` }],
					details,
				};
			}

			state.logger.lifecycle("status report accepted", {
				command,
				detail: {
					status: params.status,
					summary: params.summary,
					nextStepCount: params.next_steps?.length ?? 0,
					recommendedNextStep: params.recommended_next_step,
				},
			});

			recordCommandStatus(pi, command, params.status, params.summary);

			const next =
				params.recommended_next_step === undefined
					? params.next_steps?.[0]
					: params.next_steps?.[params.recommended_next_step];
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
