/**
 * report_scramjet_command_status tool and dormant command notice.
 *
 * The tool is the agent's structured report at the end of an active Scramjet
 * command, supplied in a *separate* turn from the command's user-facing answer
 * (issue 84, two-phase protocol).
 *
 * Four statuses, three execution paths:
 * - "continuing" during a probe: non-terminating. Increments continueCount,
 *   re-arms the probe, returns without terminate so the agent keeps working.
 * - "continuing" while dormant: non-terminating. Resets continueCount, re-arms
 *   the probe, returns without terminate. This is the only dormant resume path.
 * - "completed" / "blocked" / "incomplete": terminating. Accepted only while
 *   probe is in flight. Stores the report in lastReport and returns
 *   terminate: true. auto-continue.ts reads the stored status on the probe
 *   turn's agent_end and validates/dispatches/pauses.
 *
 * The dormant notice is a volatile system prompt section that tells the agent
 * about a dormant command and how to resume it (via `continuing`).
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
	"report_scramjet_command_status is not active right now. Do not call this tool for ordinary tasks — " +
	"call it only when Scramjet's status-check message explicitly asks you to report command status.";

const TERMINAL_FROM_DORMANT_ERROR =
	"Terminal status reports (completed/blocked/incomplete) are only accepted during a Scramjet status probe. " +
	'The command is currently dormant. To resume work, call this tool with status: "continuing" first — ' +
	"that re-arms the probe cycle, and you can report a terminal status on the next probe.";

const CONTINUE_LIMIT_ERROR =
	`You have reported "continuing" ${CONTINUE_LIMIT} times without completing the command. ` +
	"Report your actual status: completed, blocked, or incomplete.";

const OUT_OF_PHASE_ERROR =
	"report_scramjet_command_status is not active right now. Do not call this tool for ordinary tasks — " +
	"call it only when Scramjet's status-check message explicitly asks you to report command status.";

// F6: single source of truth for the next-step wire shape.
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
		'To resume work on it, call `report_scramjet_command_status` with `status: "continuing"`.\n' +
		"Terminal statuses (completed/blocked/incomplete) are only accepted during a Scramjet status probe — " +
		"call `continuing` first to re-enter the probe cycle, then report your terminal status on the next probe."
	);
}

export function registerCommandStatusTool(pi: ExtensionAPI, state: ScramjetState) {
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

	pi.registerTool({
		name: "report_scramjet_command_status",
		label: "Report Scramjet Command Status",
		description:
			"Report the status of an active Scramjet slash command after Scramjet explicitly asks for a status check. " +
			"Do not call this tool for ordinary user tasks. Do not call it unless the latest message asks you to call it.",
		parameters: Type.Object({
			status: STATUS_SCHEMA,
			summary: Type.String({ description: "Brief summary of the command's outcome." }),
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
					const details: CommandStatusDetails = { status: "continuing", summary: params.summary };
					return {
						content: [{ type: "text", text: "Continuing. Proceed with your work." }],
						details,
					};
				}

				// Dormant continuing
				if (canAcceptDormantContinuing(state.lifecycle)) {
					state.logger.lifecycle("dormant continuing accepted", {
						command,
						detail: { summary: params.summary },
					});
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
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
					details,
				};
			}

			// Terminal status path — only accepted during probe in flight
			if (!canAcceptTerminalReport(state.lifecycle)) {
				if (isDormant(state.lifecycle)) {
					state.logger.lifecycle("status report rejected", {
						command,
						detail: { reason: "terminal-from-dormant", status: params.status },
					});
					const details: CommandStatusDetails = { error: "terminal-from-dormant" };
					return {
						content: [{ type: "text", text: TERMINAL_FROM_DORMANT_ERROR }],
						details,
					};
				}
				state.logger.lifecycle("status report rejected", {
					command,
					detail: { reason: "out-of-phase", status: params.status },
				});
				state.logger.warn("status", "report_scramjet_command_status called out of phase; report ignored", {});
				const details: CommandStatusDetails = { error: "out-of-phase" };
				return {
					content: [{ type: "text", text: OUT_OF_PHASE_ERROR }],
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
					content: [{ type: "text", text: "Status report failed." }],
					details,
					terminate: true,
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

			recordCommandStatus(pi, command, params.status);

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
