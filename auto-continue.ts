/**
 * Auto-continuation: on agent_end, read the active command's next-step
 * policy, show a 3s countdown widget (cancellable by Escape or any
 * keypress), then dispatch the chosen command via sendUserMessage or
 * /scramjet-exec-fresh. `forced` fires unconditionally; the rest defer
 * to /scramjet on|off. See CLAUDE.md "MVP design rationales" for why.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { matchesKey } from "@earendil-works/pi-tui";
import { validateNextStep } from "./commands/validator.ts";
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
		// Reconstruct the slash-form wire payload from the structured NextStep.
		// The agent supplies bare `name` (matched against bare candidate names
		// by the validator) and optional `args`; the dispatcher owns the slash
		// prefix and the join. Keeping the two responsibilities split prevents
		// the F15 "is the leading slash part of the name?" ambiguity. (F15)
		return `/${step.name}${step.args ? ` ${step.args}` : ""}`;
	}

	function startCountdown(step: NextStep, ctx: ExtensionContext) {
		if (!ctx.hasUI) {
			executeStep(step);
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
					executeStep(step);
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

	function executeStep(step: NextStep) {
		const wire = wireFor(step);
		if (step.freshSession) {
			pi.sendUserMessage(`/scramjet-exec-fresh ${wire}`, { deliverAs: "followUp" });
		} else {
			pi.sendUserMessage(wire, { deliverAs: "followUp" });
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

	function dispatchForced(target: string, ctx: ExtensionContext): boolean {
		// F6: symmetric to the F11 "active command missing from registry" guard
		// at the top of agent_end. A `forced` target that dropped out of the
		// registry (rename, removed command, partial reload) would otherwise
		// silently dispatch `/dead-command` and set activeTopLevelCommand to a
		// non-registry name; the next agent_end would then fall back to the
		// legacy path with no signal that the forced chain went off the rails.
		if (!state.registry.has(target)) {
			ctx.ui.notify(`scramjet: forced target "${target}" not in registry; auto-continue skipped`, "warning");
			return false;
		}
		// Flag history's input handler to mark the resulting sidebar entry
		// origin: "forced". Cleared by that handler once it matches. The flag
		// value is the bare command name (no slash) because parseSlashCommand
		// strips the slash before returning. The wire message needs the slash
		// so Pi routes it as a slash command, not as plain user text.
		state.pendingForcedDispatch = target;
		state.activeTopLevelCommand = target;
		pi.sendUserMessage(`/${target}`, { deliverAs: "followUp" });
		return true;
	}

	pi.on("agent_end", async (_event, ctx) => {
		const activeName = state.activeTopLevelCommand;
		const def = activeName ? state.registry.get(activeName) : undefined;

		// F11: activeTopLevelCommand is set but the registry has no matching
		// entry. This used to silently fall through to the legacy auto-continue
		// path, which means a `forced` chain whose target dropped out of the
		// registry (e.g. a renamed command, a partial reload) became silently
		// un-forced. Notify the user and bail; clear the stale name so the
		// warning fires once instead of on every subsequent agent_end.
		if (activeName && !def) {
			ctx.ui.notify(`scramjet: active command "${activeName}" not in registry; auto-continue skipped`, "warning");
			state.activeTopLevelCommand = null;
			clearLatestCompletion();
			return;
		}

		const policy = def?.next;

		// Forced fires the target unconditionally — regardless of state.enabled,
		// regardless of whether the agent called task_complete. The user
		// implicitly chose to chain by invoking the command that declares forced.
		if (policy?.mode === "forced") {
			clearLatestCompletion();
			dispatchForced(policy.target, ctx);
			return;
		}

		const completion = getLatestCompletion();
		if (!completion) return;
		clearLatestCompletion();

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

		if (!state.enabled) return;
		if (!completion.nextStep) return;
		startCountdown(completion.nextStep, ctx);
	});

	// Clean up on session shutdown
	pi.on("session_shutdown", async (_event, ctx) => {
		cancelCountdown(ctx);
	});
}
