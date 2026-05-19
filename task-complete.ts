/**
 * The task_complete tool and system prompt injection.
 *
 * When a command's instructions suggest a next step, Claude reports it
 * via this tool in a structured form Scramjet can act on.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { buildNextStepBlock } from "./next-step.ts";
import type { CompletionSignal, ScramjetState } from "./types.ts";

export const NEXT_STEP_MESSAGE_TYPE = "scramjet-next-step";

const SYSTEM_PROMPT_SNIPPET = `

## Task Completion

When you have fully completed the current task — all actions taken, all user questions resolved, final output delivered — call the \`task_complete\` tool.

If the task's instructions suggest a next step (e.g., "Next: /clear then /some-command ..."), include it in the \`next_step\` field. Set \`fresh_session\` to true if the instructions say to start a fresh session (indicated by "/clear then ..." or "in a fresh session"). Set it to false if the next step should continue in the current session.

Rules:
- Do NOT call task_complete while you still have questions for the user or work remaining.
- Do NOT invent next steps — only include one if the task's instructions explicitly suggest it.
- If the task has no suggested next step, omit the next_step field entirely.
`;

export interface TaskCompleteParams {
	summary: string;
	next_step?: {
		command: string;
		fresh_session: boolean;
		reason?: string;
	};
}

export function paramsToCompletionSignal(params: TaskCompleteParams): CompletionSignal {
	return {
		summary: params.summary,
		nextStep: params.next_step
			? {
					command: params.next_step.command,
					freshSession: params.next_step.fresh_session,
					reason: params.next_step.reason,
				}
			: undefined,
	};
}

let latestCompletion: CompletionSignal | null = null;

export function getLatestCompletion(): CompletionSignal | null {
	return latestCompletion;
}

export function clearLatestCompletion(): void {
	latestCompletion = null;
}

export function registerTaskCompleteTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "task_complete",
		label: "Task Complete",
		description:
			"Signal that the current task is fully complete. Call this when all work is done and all user questions are resolved. Optionally include a recommended next step if the task's instructions suggest one.",
		parameters: Type.Object({
			summary: Type.String({ description: "Brief summary of what was accomplished" }),
			next_step: Type.Optional(
				Type.Object({
					command: Type.String({
						description: "The next command to run, e.g. '/mach10:issue-plan 55'",
					}),
					fresh_session: Type.Boolean({
						description:
							"Whether to start a fresh session first (true if instructions say '/clear then ...' or 'in a fresh session')",
					}),
					reason: Type.Optional(
						Type.String({ description: "Brief explanation of why this is the recommended next step" }),
					),
				}),
			),
		}),
		async execute(_toolCallId, params) {
			latestCompletion = paramsToCompletionSignal(params);

			// Result text differs by case for transcript readability:
			// - With next_step: forward-looking pointer (matches the countdown widget,
			//   useful when reviewing a session after the widget is gone).
			// - Without next_step: just the summary, no completion ceremony — the
			//   `task_complete` call header already signals "this was the end."
			const text = params.next_step ? `→ ${params.next_step.command}` : `Task Summary: ${params.summary}`;

			return {
				content: [{ type: "text", text }],
				details: { summary: params.summary, ...(params.next_step ?? {}) },
				terminate: true,
			};
		},
	});

	pi.on("before_agent_start", async (event) => {
		const def = state.activeTopLevelCommand ? state.registry.get(state.activeTopLevelCommand) : undefined;
		const policy = def?.next;

		// When the active command declares a policy, surface it to the agent
		// regardless of state.enabled. /off gates dispatch decisions, not the
		// agent's awareness of what the policy is — the agent's pick is still
		// needed for the notify-hint path under /off.
		if (policy) {
			clearLatestCompletion();
			return {
				systemPrompt: event.systemPrompt + SYSTEM_PROMPT_SNIPPET,
				message: {
					customType: NEXT_STEP_MESSAGE_TYPE,
					content: buildNextStepBlock(policy, def.name),
					display: false,
				},
			};
		}

		// Legacy path: no declared policy, snippet only when enabled.
		if (!state.enabled) return;
		clearLatestCompletion();
		return {
			systemPrompt: event.systemPrompt + SYSTEM_PROMPT_SNIPPET,
		};
	});
}
