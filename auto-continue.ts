/**
 * Auto-continuation, two-phase command-status protocol (issue 84).
 *
 * A top-level Scramjet command produces its normal user-facing answer in one
 * turn (the answer turn injects nothing about completion). On that turn's
 * agent_end, if the command declares a next-step policy, this driver advances
 * the lifecycle to "probing" and DEFERS a hidden status-check probe — a custom
 * message that triggers a short second turn in which the agent calls
 * report_scramjet_command_status. The tool records the status and advances the phase
 * to "reported"; this driver reads it on the probe turn's agent_end and
 * validates/dispatches/pauses.
 *
 * The probe MUST be deferred, not sent synchronously from the agent_end
 * listener: during agent_end the run is still streaming, so a synchronous
 * sendMessage routes to steer/followUp and is dropped by the already-exited
 * loop. A setTimeout(0) lands after the run settles — isStreaming clears when
 * agent.prompt() resolves — so triggerTurn correctly reaches agent.prompt().
 * This mirrors why the countdown (setInterval) dispatch works — it fires once
 * the run is idle.
 *
 * The completed-transition dispatch MUST be deferred for the same reason (issue
 * 88 duplicate-dispatch incident). Calling dispatchUserInput synchronously from
 * the probe turn's agent_end runs it while Pi still counts the run as streaming,
 * so Pi expands the slash command and queues the body as a follow-up — but the
 * agent loop has already passed its follow-up polling point for the just-ending
 * run, so the expanded body lingers stale in the queue and is delivered on a
 * later unrelated turn (a duplicate command body with no command-start). The
 * single routeCompleted call site is therefore scheduled on a deferred tick
 * (scheduleCompletedDispatch), which also covers the no-UI closed/open path that
 * dispatches immediately rather than through the selector.
 *
 * For completed commands: `forced` fires the declared target unconditionally
 * after completion; closed/open validate selector-visible options. With UI,
 * Scramjet shows a selector and `/scramjet on` auto-selects a recommended
 * command after a 3s countdown; without UI it dispatches only a valid
 * recommended command under `/scramjet on`. Dispatch uses Pi's
 * dispatchUserInput so slash commands run through Pi's normal input pipeline.
 * See CLAUDE.md "MVP design rationales".
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { COMMAND_STATUS_PROBE_TYPE } from "./command-status.ts";
import { parseSlashCommand, type ValidatedNextStep, validateNextSteps } from "./commands/validator.ts";
import { buildProbeMessage } from "./next-step.ts";
import { dispatchNextStep } from "./next-step-dispatch.ts";
import { selectNextStep } from "./next-step-selector.ts";
import { transitionPhase } from "./phase-machine.ts";
import type { CommandStatusNextStep, NextStep, NextStepPolicy, ScramjetState } from "./types.ts";

const COUNTDOWN_SECONDS = 3;
// F1: liveness watchdog window. Generous on purpose — a live probe turn is a
// single report_scramjet_command_status tool call and reports well within this, and the
// guard inside the timer re-checks the phase so a turn that DID complete (or
// already self-healed) is never clobbered. The value only bounds how long a
// probe that never produced a turn at all (dropped triggerTurn during run
// settle, Escape before the turn starts, session teardown mid-turn) lingers at
// "probing" before self-healing; the next real command resets the phase anyway.
// Kept comfortably longer than any plausible probe turn so it cannot fire while
// the model is still thinking before its tool call (phase still "probing"),
// which would otherwise drop a legitimate chain — worse than the stall it fixes.
const PROBE_WATCHDOG_MS = 30_000;

// Parse a raw report entry's message into a dispatchable NextStep, or null
// when the message is not a slash command. Used for the forced-handoff path,
// which reads the raw payload before selector validation.
function toNextStep(step: CommandStatusNextStep | undefined): NextStep | undefined {
	if (!step) return undefined;
	const parsed = parseSlashCommand(step.message);
	if (!parsed) return undefined;
	return { name: parsed.name, args: parsed.args, freshSession: step.fresh_session ?? false, reason: step.reason };
}

function selectorErrorMessage(err: unknown): string {
	try {
		return err instanceof Error ? err.message : String(err);
	} catch {
		return "<non-stringifiable rejection>";
	}
}

function isExpectedSelectorCancellation(err: unknown): boolean {
	if (!(err instanceof Error)) return false;
	return err.name === "AbortError" || err.name === "CanceledError" || err.name === "CancelledError";
}

// S2: model-supplied summary/prompt text is interpolated into ctx.ui.notify,
// which renders on a single line. Strip control chars (newlines included),
// collapse internal whitespace, and cap the length so a multi-paragraph or
// control-char report can't garble the widget. Mirrors next-step.ts formatHint's
// trim+collapse, plus a length cap since a status summary is unbounded. No
// safe() close-tag escaping is needed — notify text is never re-injected into a
// prompt.
// Exported for direct unit testing of the boundary (NOTIFY_MAX - 1 + "…") and
// the control-char/whitespace passes; the production callers are routeNonCompleted's
// blocked/waiting notifies.
export const NOTIFY_MAX = 200;
export function cleanForNotify(text: string): string {
	const collapsed = text
		.replace(/[\u0000-\u001f\u007f]/g, " ")
		.trim()
		.replace(/\s+/g, " ");
	return collapsed.length > NOTIFY_MAX ? `${collapsed.slice(0, NOTIFY_MAX - 1)}…` : collapsed;
}

export function registerAutoContinue(pi: ExtensionAPI, state: ScramjetState) {
	let probeTimer: ReturnType<typeof setTimeout> | null = null;
	let probeWatchdog: ReturnType<typeof setTimeout> | null = null;
	let dispatchTimer: ReturnType<typeof setTimeout> | null = null;
	let activeSelectorId = 0;
	let activeSelectorAbort: AbortController | null = null;

	function clearProbeWatchdog() {
		if (probeWatchdog) {
			clearTimeout(probeWatchdog);
			probeWatchdog = null;
		}
	}

	state.suspendProbeWatchdog = () => clearProbeWatchdog();
	state.rearmProbeWatchdog = () => {
		clearProbeWatchdog();
		if (state.commandPhase === "probing") {
			probeWatchdog = setTimeout(() => {
				probeWatchdog = null;
				if (state.commandPhase === "probing") {
					transitionPhase(state, "idle");
					console.warn("scramjet: status probe turn never completed; auto-continue paused");
				}
			}, PROBE_WATCHDOG_MS);
		}
	};

	function clearDispatchTimer() {
		if (dispatchTimer) {
			clearTimeout(dispatchTimer);
			dispatchTimer = null;
		}
	}

	function executeStep(step: NextStep, ctx: ExtensionContext) {
		dispatchNextStep(ctx, state, step, { origin: "agent" });
	}

	// Build the dispatchable NextStep for a validated option whose message
	// parsed as a slash command.
	function toDispatchStep(option: ValidatedNextStep, parsed: NonNullable<ValidatedNextStep["parsedCommand"]>) {
		return { name: parsed.name, args: parsed.args, freshSession: option.freshSession, reason: option.reason };
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
		clearProbeWatchdog();
		const content = buildProbeMessage(policy, commandId, state.enabled);
		probeTimer = setTimeout(() => {
			probeTimer = null;
			// Error boundary, symmetric to the countdown setInterval guard above.
			// sendMessage returns void, so a throw on this deferred tick becomes a
			// Node uncaughtException and leaves commandPhase wedged at "probing"
			// with no live probe behind it — self-healing only if another agent_end
			// happens to fire later. Reset the lifecycle so the chain pauses cleanly
			// instead of stalling. ctx is out of scope here, so warn to the console
			// rather than ctx.ui.notify.
			try {
				pi.sendMessage({ customType: COMMAND_STATUS_PROBE_TYPE, content, display: false }, { triggerTurn: true });
				// F1: time-domain analog of the throw boundary above. The
				// probing→idle self-heal lives in the agent_end handler, so it only
				// fires if the probe turn emits a terminal agent_end. If the
				// triggered turn never materializes at all (dropped triggerTurn,
				// Escape before it starts, teardown mid-turn), the phase would sit at
				// "probing" until the next real command. Arm a watchdog that self-heals
				// after a generous window; the phase re-check inside keeps it from
				// clobbering a probe turn that did complete. Cleared at the probe turn's
				// agent_end (reported/probing branches) and on shutdown.
				probeWatchdog = setTimeout(() => {
					probeWatchdog = null;
					if (state.commandPhase === "probing") {
						transitionPhase(state, "idle");
						console.warn("scramjet: status probe turn never completed; auto-continue paused");
					}
				}, PROBE_WATCHDOG_MS);
			} catch (err) {
				transitionPhase(state, "idle");
				console.warn(`scramjet: status probe failed to send (${(err as Error).message}); auto-continue paused`);
			}
		}, 0);
	}

	function skippedSummary(skipped: ReturnType<typeof validateNextSteps>["skipped"]): string {
		return cleanForNotify(
			skipped.map((step) => `${cleanForNotify(step.label)} (${cleanForNotify(step.reason)})`).join(", "),
		);
	}

	function optionSummary(option: ReturnType<typeof validateNextSteps>["valid"][number]): string {
		return cleanForNotify(option.message);
	}

	function optionsSummary(options: ReturnType<typeof validateNextSteps>["valid"]): string {
		return cleanForNotify(options.map(optionSummary).join(", "));
	}

	function runSelectedOption(option: ValidatedNextStep, ctx: ExtensionContext) {
		if (option.parsedCommand) {
			executeStep(toDispatchStep(option, option.parsedCommand), ctx);
		} else {
			ctx.ui.pasteToEditor(option.message);
		}
	}

	function cancelSelector() {
		activeSelectorId++;
		activeSelectorAbort?.abort();
		activeSelectorAbort = null;
	}

	function showSelector(result: ReturnType<typeof validateNextSteps>, ctx: ExtensionContext) {
		cancelSelector();
		const selectorId = activeSelectorId;
		const controller = new AbortController();
		activeSelectorAbort = controller;
		const autoSelect = state.enabled && result.recommended?.parsedCommand ? result.recommended : undefined;
		void selectNextStep(ctx, {
			options: result.valid,
			recommended: result.recommended,
			autoSelect,
			countdownSeconds: autoSelect ? COUNTDOWN_SECONDS : 0,
			signal: controller.signal,
		})
			.then((selected) => {
				if (selectorId !== activeSelectorId) return;
				activeSelectorAbort = null;
				if (selected) runSelectedOption(selected, ctx);
			})
			.catch((err) => {
				if (selectorId !== activeSelectorId) {
					if (!isExpectedSelectorCancellation(err)) {
						console.warn(
							`scramjet: stale next-step selector failed (${selectorErrorMessage(err)}); failure ignored`,
						);
					}
					return;
				}
				activeSelectorAbort = null;
				ctx.ui.notify(
					`scramjet: next-step selector failed (${selectorErrorMessage(err)}); auto-continue paused`,
					"warning",
				);
			});
	}

	function routeWithoutUi(result: ReturnType<typeof validateNextSteps>, ctx: ExtensionContext) {
		if (!result.recommended) {
			if (result.recommendedReason) {
				const level = state.enabled && result.valid.every((option) => !option.parsedCommand) ? "info" : "warning";
				ctx.ui.notify(
					`scramjet: ${result.recommendedReason}; valid option(s): ${optionsSummary(result.valid)}`,
					level,
				);
			}
			return;
		}

		if (!result.recommended.parsedCommand) {
			const text = optionSummary(result.recommended);
			if (state.enabled) {
				ctx.ui.notify(
					`scramjet: recommended next step is not a slash command (${text}); automatic dispatch skipped`,
					"warning",
				);
			} else {
				ctx.ui.notify(
					`scramjet: next would be ${text}; /scramjet on only auto-dispatches command next steps`,
					"info",
				);
			}
			return;
		}

		if (state.enabled) {
			executeStep(toDispatchStep(result.recommended, result.recommended.parsedCommand), ctx);
		} else {
			const fresh = result.recommended.freshSession ? " (fresh session)" : "";
			ctx.ui.notify(
				`scramjet: next would be ${cleanForNotify(result.recommended.message)}${fresh}; /scramjet on to chain`,
				"info",
			);
		}
	}

	// Route a completed-status report through the command's policy. Mirrors the
	// pre-84 dispatch logic, now reading the validated recommendation from the
	// next_steps[] array rather than a single next_step.
	function routeCompleted(
		policy: NextStepPolicy,
		status: NonNullable<ScramjetState["latestCommandStatus"]>,
		ctx: ExtensionContext,
	) {
		if (policy.mode === "forced") {
			// Forced fires regardless of state.enabled: no decision is delegated to
			// the agent or user; the status report is only the safety gate that
			// distinguishes completion from clarification/error. A handoff whose
			// message is not a slash command parses to undefined and is ignored.
			const handoff = toNextStep(status.next_steps?.[0]);
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

		const result = validateNextSteps(status.next_steps, policy, status.recommended_next_step);
		if (!result.valid.length) {
			if (result.reason) ctx.ui.notify(`scramjet: ${result.reason}`, "warning");
			return;
		}

		if (result.skipped.length) {
			ctx.ui.notify(`scramjet: skipped invalid next step(s): ${skippedSummary(result.skipped)}`, "info");
		}

		if (ctx.hasUI) {
			if (result.recommendedReason) {
				const level = state.enabled && result.valid.every((option) => !option.parsedCommand) ? "info" : "warning";
				ctx.ui.notify(
					`scramjet: ${result.recommendedReason}; valid option(s): ${optionsSummary(result.valid)}`,
					level,
				);
			}
			showSelector(result, ctx);
		} else {
			routeWithoutUi(result, ctx);
		}
	}

	// Dispatch a completed transition on a deferred tick, mirroring scheduleProbe.
	// routeCompleted's forced path (and its no-UI closed/open path) call
	// dispatchUserInput; running that synchronously from the probe turn's agent_end
	// queues a stale, duplicate command body (see the file header). A setTimeout(0)
	// lands after the run settles so the next command dispatches once, as a clean
	// new turn. ctx is captured and remains valid across the deferral — the same
	// property the countdown's setInterval ticks already rely on.
	function scheduleCompletedDispatch(
		policy: NextStepPolicy,
		status: NonNullable<ScramjetState["latestCommandStatus"]>,
		ctx: ExtensionContext,
	) {
		if (dispatchTimer) clearTimeout(dispatchTimer);
		dispatchTimer = setTimeout(() => {
			dispatchTimer = null;
			// Error boundary, symmetric to scheduleProbe's: a throw on this deferred
			// tick would otherwise become a Node uncaughtException. ctx is in scope
			// here (unlike scheduleProbe), so surface the failure through the UI and
			// pause the chain cleanly.
			try {
				routeCompleted(policy, status, ctx);
			} catch (err) {
				ctx.ui.notify(
					`scramjet: next-step dispatch failed (${(err as Error).message}); auto-continue paused`,
					"warning",
				);
			}
		}, 0);
	}

	// A non-completed report never chains; the differentiation is only in what
	// gets surfaced. `blocked` merits a warning (the command hit an error,
	// missing dependency, or authorization issue the user should see). For
	// `waiting_for_user` the visible assistant answer already asked the question,
	// so we only echo `user_prompt` as an info hint when the agent supplied one.
	// `incomplete` is a quiet pause — staying invisible when there's nothing
	// useful to say (see CLAUDE.md "Invisible when idle").
	function routeNonCompleted(status: NonNullable<ScramjetState["latestCommandStatus"]>, ctx: ExtensionContext) {
		switch (status.status) {
			case "blocked":
				ctx.ui.notify(`scramjet: command blocked — ${cleanForNotify(status.summary)}`, "warning");
				return;
			case "waiting_for_user":
				if (status.user_prompt) {
					ctx.ui.notify(`scramjet: waiting for input — ${cleanForNotify(status.user_prompt)}`, "info");
				}
				return;
			default:
				// incomplete: quiet pause.
				return;
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
			transitionPhase(state, "idle");
			clearProbeWatchdog();
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
					transitionPhase(state, "idle");
					return;
				}
				if (!transitionPhase(state, "probing")) return;
				scheduleProbe(policy, def.name);
				return;
			}
			case "probing": {
				// The probe turn ended without a recorded report_scramjet_command_status call:
				// either the agent wrote prose instead of reporting, or it DID call the
				// tool but Pi rejected the call on schema grounds before `execute` ran
				// (missing required field, bad status literal) so the phase never
				// advanced to "reported". Self-heal: pause the chain, reset, do not
				// re-probe — no infinite loop. The turn produced a terminal agent_end,
				// so the liveness watchdog is no longer needed.
				//
				// F1: this is otherwise the only self-heal in the protocol that resets
				// silently; every sibling (watchdog, send-throw, out-of-phase gate in
				// command-status.ts) leaves a log breadcrumb. A subtly-malformed status
				// payload would stop the chain with no diagnostic trail. Log-only —
				// preserves "invisible when idle" on the user surface.
				clearProbeWatchdog();
				transitionPhase(state, "idle");
				console.warn("scramjet: status probe turn ended without a valid status report; auto-continue paused");
				return;
			}
			case "reported": {
				// The probe turn completed and reported, so the liveness watchdog is
				// no longer needed.
				clearProbeWatchdog();
				const status = state.latestCommandStatus;
				// The stored report is consumed regardless of outcome; the resting
				// phase is then set per status below (terminal idle, or resumable
				// waiting for waiting_for_user). Cleared explicitly before routing
				// because the target may be "waiting" (where auto-clear doesn't apply).
				state.latestCommandStatus = null;
				if (!status) {
					transitionPhase(state, "idle");
					return;
				}
				if (status.status === "completed") {
					// Completion is terminal for the lifecycle: reset to idle, then chain.
					transitionPhase(state, "idle");
					// issue 128: clear activeTopLevelCommand so a later interactive
					// reply doesn't re-arm the phase for a completed command.
					// routeCompleted/scheduleCompletedDispatch use captured `policy`
					// and `status` parameters, not activeTopLevelCommand; the next
					// command start (if chaining) sets it to the new command.
					state.activeTopLevelCommand = null;
					// Chaining a completed command needs the policy to validate the
					// pick (or fire the forced target). If def.next vanished between
					// the probe and this agent_end (registry rebuild/reload), there is
					// nothing to route a completed status to — pause quietly. The
					// dispatch is deferred off this agent_end tick (see the file header):
					// dispatching synchronously while the run is still streaming queues a
					// stale duplicate command body.
					if (policy) scheduleCompletedDispatch(policy, status, ctx);
					return;
				}
				// F2: a non-completed report (blocked/waiting_for_user/incomplete)
				// never chains, so it does not depend on the policy. Route it even
				// when policy is gone so a `blocked` summary (auth failure, missing
				// dependency) still surfaces instead of being swallowed when def.next
				// disappeared.
				//
				// issue 88: waiting_for_user is a *resumable* halt — park at "waiting"
				// (keeping activeTopLevelCommand) so a later interactive reply
				// (history.ts) can re-arm the running→probing probe path and the
				// command can later report completed. blocked/incomplete stay terminal
				// (idle), so they clear the active command like completed does.
				if (!transitionPhase(state, status.status === "waiting_for_user" ? "waiting" : "idle")) return;
				if (status.status !== "waiting_for_user") state.activeTopLevelCommand = null;
				routeNonCompleted(status, ctx);
				return;
			}
			case "waiting": {
				// issue 88: a paused (waiting_for_user) command rests here between the
				// probe that reported it and the user's reply. A stray agent_end while
				// still "waiting" — a turn NOT preceded by an interactive resume, which
				// history.ts flips waiting→running on the reply — must NOT fire a probe,
				// or it would re-probe with no user answer behind it. Stable no-op.
				return;
			}
			default:
				return;
		}
	});

	// Clean up on session shutdown: drop any scheduled-but-unfired probe or deferred completed-transition dispatch.
	pi.on("session_shutdown", async () => {
		cancelSelector();
		if (probeTimer) {
			clearTimeout(probeTimer);
			probeTimer = null;
		}
		clearProbeWatchdog();
		clearDispatchTimer();
	});
}
