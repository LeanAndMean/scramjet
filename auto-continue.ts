/**
 * Auto-continuation: on agent_end, read the active command's next-step
 * policy, show a 3s countdown widget (cancellable by Escape or any
 * keypress), then dispatch the chosen command. `forced` fires after the
 * agent signals completion; the rest defer to /scramjet on|off. See
 * CLAUDE.md "MVP design rationales" for why.
 *
 * Dispatch uses Pi's experimental dispatchUserInput primitive so slash
 * commands, skills, and prompt templates run through Pi's normal input
 * pipeline instead of Scramjet expanding command bodies locally.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { matchesKey } from "@earendil-works/pi-tui";
import { validateNextStep } from "./commands/validator.ts";
import { buildNextStepWire, dispatchNextStep } from "./next-step-dispatch.ts";
import { clearLatestCompletion, getLatestCompletion } from "./task-complete.ts";
import type { NextStep, ScramjetState } from "./types.ts";

const COUNTDOWN_SECONDS = 3;
const WIDGET_KEY = "scramjet-next";

export function registerAutoContinue(pi: ExtensionAPI, state: ScramjetState) {
	let countdownTimer: ReturnType<typeof setInterval> | null = null;
	let unsubInput: (() => void) | null = null;

	function cancelCountdown(ctx: ExtensionContext) {
		if (countdownTimer) {
			clearInterval(countdownTimer);
			countdownTimer = null;
		}
		if (unsubInput) {
			unsubInput();
			unsubInput = null;
		}
		ctx.ui.setWidget(WIDGET_KEY, undefined);
	}

	function wireFor(step: NextStep): string {
		return buildNextStepWire(step);
	}

	function startCountdown(step: NextStep, ctx: ExtensionContext) {
		if (!ctx.hasUI) {
			executeStep(step, ctx);
			return;
		}

		let remaining = COUNTDOWN_SECONDS;
		const wire = wireFor(step);

		const updateWidget = () => {
			const sessionLabel = step.freshSession ? " (fresh session)" : "";
			const dots = ".".repeat(remaining);
			ctx.ui.setWidget(WIDGET_KEY, [`  Next: ${wire}${sessionLabel}    ${remaining}s${dots}    [Esc] cancel  `], {
				placement: "belowEditor",
			});
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
			// S10: error boundary. setInterval callbacks that throw become
			// unhandledException in Node — worse, the interval keeps firing.
			// Catch, surface, and tear the countdown down cleanly so a bad
			// tick doesn't trap the user in a runaway widget.
			try {
				remaining--;
				if (remaining <= 0) {
					cancelCountdown(ctx);
					executeStep(step, ctx);
				} else {
					updateWidget();
				}
			} catch (err) {
				cancelCountdown(ctx);
				ctx.ui.notify(
					`scramjet: countdown aborted (${(err as Error).message}); press the next-step command manually if you still want to chain`,
					"warning",
				);
			}
		}, 1000);
	}

	function executeStep(step: NextStep, ctx: ExtensionContext) {
		dispatchNextStep(ctx, state, step, { origin: "agent" });
	}

	function dispatchForced(target: string, handoff: NextStep | undefined, ctx: ExtensionContext): boolean {
		// F6: symmetric to the F11 "active command missing from registry" guard
		// at the top of agent_end. A `forced` target that dropped out of the
		// registry (rename, removed command, partial reload) would otherwise
		// silently dispatch the wrong command and set activeTopLevelCommand to
		// a non-registry name; the next agent_end would then warn late, after the
		// forced chain already went off the rails.
		const def = state.registry.get(target);
		if (!def) {
			ctx.ui.notify(`scramjet: forced target "${target}" not in registry; auto-continue skipped`, "warning");
			return false;
		}

		let step: NextStep = { name: target, freshSession: false };
		if (handoff) {
			if (handoff.name === target) {
				step = { ...handoff, name: target };
			} else {
				ctx.ui.notify(
					`scramjet: forced target is "${target}"; agent supplied next_step.name "${handoff.name}" — ignoring supplied forced handoff`,
					"warning",
				);
			}
		}

		dispatchNextStep(ctx, state, step, { origin: "forced" });
		return true;
	}

	pi.on("agent_end", async (_event, ctx) => {
		const activeName = state.activeTopLevelCommand;
		const def = activeName ? state.registry.get(activeName) : undefined;

		// F11: activeTopLevelCommand is set but the registry has no matching
		// entry. Without this guard, a `forced` chain whose target dropped out of
		// the registry (e.g. a renamed command, a partial reload) would be hard to
		// diagnose. Notify the user and bail; clear the stale name so the warning
		// fires once instead of on every subsequent agent_end.
		if (activeName && !def) {
			ctx.ui.notify(`scramjet: active command "${activeName}" not in registry; auto-continue skipped`, "warning");
			state.activeTopLevelCommand = null;
			clearLatestCompletion();
			return;
		}

		const policy = def?.next;
		const completion = getLatestCompletion();
		if (!completion) return;
		clearLatestCompletion();

		// Forced fires the target after the agent explicitly signals that the
		// command completed. It still ignores state.enabled because no decision is
		// being delegated to the agent or user; task_complete is only the safety
		// gate that distinguishes successful completion from clarification/error.
		if (policy?.mode === "forced") {
			dispatchForced(policy.target, completion.nextStep, ctx);
			return;
		}

		if (policy) {
			const proposed = completion.nextStep?.name;
			const result = validateNextStep(proposed, policy);

			if (policy.mode === "ask") {
				if (proposed) {
					ctx.ui.notify(
						`scramjet: ask-mode command; agent proposed "${proposed}" — ignored, waiting for user`,
						"warning",
					);
				}
				return;
			}

			if (!result.valid) {
				ctx.ui.notify(`scramjet: ${result.reason}`, "warning");
				return;
			}

			if (!completion.nextStep) return;

			if (state.enabled) {
				startCountdown(completion.nextStep, ctx);
			} else {
				const wire = wireFor(completion.nextStep);
				const fresh = completion.nextStep.freshSession ? " (fresh session)" : "";
				ctx.ui.notify(`scramjet: next would be ${wire}${fresh}; /scramjet on to chain`, "info");
			}
			return;
		}

		// No declared next-step policy is equivalent to ask-with-no-hint: the
		// chain pauses, even if an agent supplied a legacy/free-form next_step.
		return;
	});

	// Clean up on session shutdown
	pi.on("session_shutdown", async (_event, ctx) => {
		cancelCountdown(ctx);
	});
}
