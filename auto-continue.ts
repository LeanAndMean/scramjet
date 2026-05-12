/**
 * Auto-continuation: after task_complete fires with a next_step,
 * show a countdown widget and auto-run the next command.
 *
 * The user can cancel by pressing Escape or typing anything.
 * If cancelled, the widget disappears and they're back in normal Pi.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { matchesKey } from "@earendil-works/pi-tui";
import { getLatestCompletion, clearLatestCompletion } from "./task-complete.ts";
import type { ScramjetState, NextStep } from "./types.ts";

const COUNTDOWN_SECONDS = 3;
const WIDGET_KEY = "scramjet-next";

export function registerAutoContinue(pi: ExtensionAPI, state: ScramjetState) {
	let countdownTimer: ReturnType<typeof setInterval> | null = null;
	let unsubInput: (() => void) | null = null;
	let pendingStep: NextStep | null = null;

	function cancelCountdown(ctx: { ui: { setWidget: (key: string, lines: string[] | undefined) => void } }) {
		if (countdownTimer) {
			clearInterval(countdownTimer);
			countdownTimer = null;
		}
		if (unsubInput) {
			unsubInput();
			unsubInput = null;
		}
		pendingStep = null;
		ctx.ui.setWidget(WIDGET_KEY, undefined);
	}

	function startCountdown(
		step: NextStep,
		ctx: {
			hasUI: boolean;
			isIdle: () => boolean;
			ui: {
				setWidget: (key: string, lines: string[] | undefined, options?: { placement: string }) => void;
				onTerminalInput: (handler: (data: string) => { consume?: boolean } | void) => () => void;
				notify: (msg: string, type: "info" | "warning" | "error") => void;
			};
		},
	) {
		if (!ctx.hasUI) {
			executeStep(step);
			return;
		}

		pendingStep = step;
		let remaining = COUNTDOWN_SECONDS;

		const updateWidget = () => {
			const sessionLabel = step.freshSession ? " (fresh session)" : "";
			const dots = ".".repeat(remaining);
			ctx.ui.setWidget(
				WIDGET_KEY,
				[`  Next: ${step.command}${sessionLabel}    ${remaining}s${dots}    [Esc] cancel  `],
				{ placement: "belowEditor" },
			);
		};

		updateWidget();

		unsubInput = ctx.ui.onTerminalInput((data) => {
			const isEscape = matchesKey(data, "escape");
			cancelCountdown(ctx);
			if (isEscape) {
				return { consume: true };
			}
		});

		countdownTimer = setInterval(() => {
			remaining--;
			if (remaining <= 0) {
				cancelCountdown(ctx);
				executeStep(step);
			} else {
				updateWidget();
			}
		}, 1000);
	}

	function executeStep(step: NextStep) {
		if (step.freshSession) {
			pi.sendUserMessage(`/scramjet-exec-fresh ${step.command}`, { deliverAs: "followUp" });
		} else {
			pi.sendUserMessage(step.command, { deliverAs: "followUp" });
		}
	}

	// Internal command to handle fresh session transitions.
	// Tools can't call ctx.newSession(), but commands can.
	pi.registerCommand("scramjet-exec-fresh", {
		description: "Internal: execute a command in a fresh session",
		handler: async (args, ctx) => {
			const command = args.trim();
			if (!command) return;

			const result = await ctx.newSession({
				withSession: async (newCtx) => {
					newCtx.ui.notify(`Scramjet: starting ${command}`, "info");
				},
			});

			if (!result.cancelled) {
				pi.sendUserMessage(command);
			}
		},
	});

	pi.on("agent_end", async (_event, ctx) => {
		if (!state.enabled) return;

		const completion = getLatestCompletion();
		if (!completion?.nextStep) return;

		clearLatestCompletion();
		startCountdown(completion.nextStep, ctx);
	});

	// Clean up on session shutdown
	pi.on("session_shutdown", async (_event, ctx) => {
		cancelCountdown(ctx);
	});
}
