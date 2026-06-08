/**
 * Auto-continuation, two-phase command-status protocol (issue 84).
 *
 * A top-level Scramjet command produces its normal user-facing answer in one
 * turn (the answer turn injects nothing about completion). On that turn's
 * agent_end, if the command declares a next-step policy, this driver advances
 * the lifecycle to "probing" and DEFERS a hidden status-check probe — a custom
 * message that triggers a short second turn in which the agent calls
 * scramjet_command_status. The tool records the status and advances the phase
 * to "reported"; this driver reads it on the probe turn's agent_end and
 * validates/dispatches/pauses.
 *
 * The probe MUST be deferred, not sent synchronously from the agent_end
 * listener: during agent_end the run is still streaming, so a synchronous
 * sendMessage routes to steer/followUp and is dropped by the already-exited
 * loop. A setTimeout(0) lands after finishRun() clears isStreaming, so
 * triggerTurn correctly reaches agent.prompt(). This mirrors why the countdown
 * (setInterval) dispatch works — it fires once the run is idle.
 *
 * For completed commands: `forced` fires the declared target unconditionally
 * after completion; closed/open defer to /scramjet on|off and show a 3s
 * countdown widget (cancellable by Escape or any keypress) before dispatch.
 * Dispatch uses Pi's dispatchUserInput so slash commands run through Pi's
 * normal input pipeline. See CLAUDE.md "MVP design rationales".
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { matchesKey } from "@earendil-works/pi-tui";
import { COMMAND_STATUS_PROBE_TYPE } from "./command-status.ts";
import { validateNextSteps } from "./commands/validator.ts";
import { buildProbeMessage } from "./next-step.ts";
import { buildNextStepWire, dispatchNextStep } from "./next-step-dispatch.ts";
import type { CommandStatusNextStep, NextStep, NextStepPolicy, ScramjetState } from "./types.ts";

const COUNTDOWN_SECONDS = 3;
const WIDGET_KEY = "scramjet-next";

function toNextStep(step: CommandStatusNextStep): NextStep {
	return { name: step.name, args: step.args, freshSession: step.fresh_session, reason: step.reason };
}

export function registerAutoContinue(pi: ExtensionAPI, state: ScramjetState) {
	let countdownTimer: ReturnType<typeof setInterval> | null = null;
	let unsubInput: (() => void) | null = null;
	let probeTimer: ReturnType<typeof setTimeout> | null = null;

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
					`scramjet: forced target is "${target}"; agent supplied next_steps name "${handoff.name}" — ignoring supplied forced handoff`,
					"warning",
				);
			}
		}

		dispatchNextStep(ctx, state, step, { origin: "forced" });
		return true;
	}

	// Schedule the hidden status-check probe on a deferred tick so it lands after
	// the run is idle (see file header). triggerTurn starts the short probe turn;
	// display:false keeps the message out of the TUI while it still persists in
	// the journal and reaches the model as user context.
	function scheduleProbe(policy: NextStepPolicy, commandId: string) {
		if (probeTimer) clearTimeout(probeTimer);
		const content = buildProbeMessage(policy, commandId);
		probeTimer = setTimeout(() => {
			probeTimer = null;
			pi.sendMessage({ customType: COMMAND_STATUS_PROBE_TYPE, content, display: false }, { triggerTurn: true });
		}, 0);
	}

	// Route a completed-status report through the command's policy. Mirrors the
	// pre-84 dispatch logic, now reading the validated first entry of the
	// next_steps[] array rather than a single next_step.
	function routeCompleted(
		policy: NextStepPolicy,
		status: NonNullable<ScramjetState["latestCommandStatus"]>,
		ctx: ExtensionContext,
	) {
		if (policy.mode === "forced") {
			// Forced fires regardless of state.enabled: no decision is delegated to
			// the agent or user; the status report is only the safety gate that
			// distinguishes completion from clarification/error.
			const handoff = status.next_steps?.[0] ? toNextStep(status.next_steps[0]) : undefined;
			dispatchForced(policy.target, handoff, ctx);
			return;
		}

		if (policy.mode === "ask") {
			if (status.next_steps?.length) {
				ctx.ui.notify(
					"scramjet: ask-mode command; agent proposed next steps — ignored, waiting for user",
					"warning",
				);
			}
			return;
		}

		// closed / open: dispatch the first entry valid for the policy.
		const result = validateNextSteps(status.next_steps, policy);
		if (!result.valid) {
			if (result.reason) ctx.ui.notify(`scramjet: ${result.reason}`, "warning");
			return;
		}

		if (state.enabled) {
			startCountdown(result.valid, ctx);
		} else {
			const wire = wireFor(result.valid);
			const fresh = result.valid.freshSession ? " (fresh session)" : "";
			ctx.ui.notify(`scramjet: next would be ${wire}${fresh}; /scramjet on to chain`, "info");
		}
	}

	pi.on("agent_end", async (_event, ctx) => {
		const activeName = state.activeTopLevelCommand;
		const def = activeName ? state.registry.get(activeName) : undefined;

		// F11: activeTopLevelCommand is set but the registry has no matching
		// entry (renamed command, partial reload). Notify once, clear the stale
		// name and reset the lifecycle so a probe never fires for a phantom
		// command and the warning doesn't repeat on every subsequent agent_end.
		if (activeName && !def) {
			ctx.ui.notify(`scramjet: active command "${activeName}" not in registry; auto-continue skipped`, "warning");
			state.activeTopLevelCommand = null;
			state.commandPhase = "idle";
			state.latestCommandStatus = null;
			return;
		}

		const policy = def?.next;

		// The phase is the eligibility signal — set to "running" only at a depth-0
		// command start (history.ts). Keying off the phase (not delegateStack
		// depth, which delegate.ts clears at before_agent_start) is what keeps the
		// probe tied to the just-finished top-level command and silent otherwise.
		switch (state.commandPhase) {
			case "running": {
				// Answer turn ended. Eligible policy → ask for status; otherwise stay
				// invisible (terminus command, or no active command).
				if (!policy) {
					state.commandPhase = "idle";
					return;
				}
				state.commandPhase = "probing";
				scheduleProbe(policy, def.name);
				return;
			}
			case "probing": {
				// The probe turn ended without a scramjet_command_status call (the
				// agent wrote prose instead of reporting). Self-heal: pause the chain,
				// reset, do not re-probe — no infinite loop.
				state.commandPhase = "idle";
				state.latestCommandStatus = null;
				return;
			}
			case "reported": {
				const status = state.latestCommandStatus;
				state.commandPhase = "idle";
				state.latestCommandStatus = null;
				if (!status || !policy) return;
				if (status.status !== "completed") {
					// Stage 2 pauses on non-completed statuses. Stage 3 differentiates
					// waiting_for_user / blocked / incomplete notifications.
					return;
				}
				routeCompleted(policy, status, ctx);
				return;
			}
			default:
				return;
		}
	});

	// Clean up on session shutdown: tear down an in-flight countdown and drop any
	// scheduled-but-unfired probe.
	pi.on("session_shutdown", async (_event, ctx) => {
		cancelCountdown(ctx);
		if (probeTimer) {
			clearTimeout(probeTimer);
			probeTimer = null;
		}
	});
}
