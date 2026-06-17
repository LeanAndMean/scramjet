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
import { loadAutonomyConfig, resolveEdgeBehavior, validateConfig } from "./autonomy-settings.ts";
import { COMMAND_STATUS_PROBE_TYPE } from "./command-status.ts";
import { parseSlashCommand, type ValidatedNextStep, validateNextSteps } from "./commands/validator.ts";
import { buildProbeMessage } from "./next-step.ts";
import { dispatchNextStep } from "./next-step-dispatch.ts";
import { selectNextStep } from "./next-step-selector.ts";
import { getActiveCommand, type LifecycleState, transition } from "./phase-machine.ts";
import type {
	CommandStatusNextStep,
	CommandStatusPayload,
	EdgeSetting,
	NextStep,
	NextStepPolicy,
	ScramjetState,
} from "./types.ts";

const COUNTDOWN_SECONDS = 3;
// Liveness watchdog window. Generous on purpose — a live probe turn is a
// single report_scramjet_command_status tool call and reports well within this,
// and the guard inside the timer re-checks the phase so a turn that DID
// complete (or already self-healed) is never clobbered. The value only bounds
// how long a probe that never produced a turn at all (dropped triggerTurn
// during run settle, Escape before the turn starts, session teardown mid-turn)
// lingers at "probing" before self-healing; the next real command resets the
// phase anyway. Kept comfortably longer than any plausible probe turn so it
// cannot fire while the model is still thinking before its tool call (phase
// still "probing"), which would otherwise drop a legitimate chain — worse than
// the stall it fixes.
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

// Model-supplied summary/prompt text is interpolated into ctx.ui.notify, which
// renders on a single line. Strip control chars (newlines included), collapse
// internal whitespace, and cap the length so a multi-paragraph or control-char
// report can't garble the widget. Mirrors next-step.ts formatHint's
// trim+collapse, plus a length cap since a status summary is unbounded. No
// safe() close-tag escaping is needed — notify text is never re-injected into a
// prompt. Exported for direct unit testing of the boundary (NOTIFY_MAX - 1 +
// "…") and the control-char/whitespace passes; the production callers are
// routeNonCompleted's blocked/waiting notifies.
export const NOTIFY_MAX = 200;
export function cleanForNotify(text: string): string {
	const collapsed = text
		.replace(/[\u0000-\u001f\u007f]/g, " ")
		.trim()
		.replace(/\s+/g, " ");
	return collapsed.length > NOTIFY_MAX ? `${collapsed.slice(0, NOTIFY_MAX - 1)}…` : collapsed;
}

function applyTransition(state: ScramjetState, result: { ok: true; state: LifecycleState }): void {
	state.lifecycle = result.state;
}

export function registerAutoContinue(pi: ExtensionAPI, state: ScramjetState) {
	let probeTimer: ReturnType<typeof setTimeout> | null = null;
	let probeWatchdog: ReturnType<typeof setTimeout> | null = null;
	let dispatchTimer: ReturnType<typeof setTimeout> | null = null;
	let activeSelectorId = 0;
	let activeSelectorAbort: AbortController | null = null;
	let autonomyValidated = false;

	state.lifecycleTimers = {
		isProbeScheduled: () => probeTimer !== null,
		isWatchdogActive: () => probeWatchdog !== null,
		isDispatchScheduled: () => dispatchTimer !== null,
	};

	function clearProbeWatchdog() {
		if (probeWatchdog) {
			clearTimeout(probeWatchdog);
			probeWatchdog = null;
		}
	}

	function armProbeWatchdog() {
		clearProbeWatchdog();
		if (state.lifecycle.phase === "probing") {
			probeWatchdog = setTimeout(() => {
				probeWatchdog = null;
				if (state.lifecycle.phase === "probing") {
					const result = transition(state.lifecycle, { type: "probe-self-healed" });
					if (result.ok) applyTransition(state, result);
					console.warn("scramjet: status probe turn never completed; auto-continue paused");
				}
			}, PROBE_WATCHDOG_MS);
		}
	}

	state.suspendProbeWatchdog = () => clearProbeWatchdog();
	state.rearmProbeWatchdog = armProbeWatchdog;

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
		// Deferred to land after the run settles; a throw here becomes an uncaughtException and leaves lifecycle wedged.
		probeTimer = setTimeout(() => {
			probeTimer = null;
			try {
				pi.sendMessage({ customType: COMMAND_STATUS_PROBE_TYPE, content, display: false }, { triggerTurn: true });
				armProbeWatchdog();
			} catch (err) {
				const result = transition(state.lifecycle, { type: "probe-self-healed" });
				if (result.ok) applyTransition(state, result);
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

	function showSelector(
		result: ReturnType<typeof validateNextSteps>,
		ctx: ExtensionContext,
		{ forcePause = false }: { forcePause?: boolean } = {},
	) {
		cancelSelector();
		const selectorId = activeSelectorId;
		const controller = new AbortController();
		activeSelectorAbort = controller;
		const autoSelect =
			!forcePause && state.enabled && result.recommended?.parsedCommand ? result.recommended : undefined;
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

	function routeWithoutUi(
		result: ReturnType<typeof validateNextSteps>,
		ctx: ExtensionContext,
		edgeSetting: EdgeSetting = null,
	) {
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

		if (edgeSetting === "pause") {
			const fresh = result.recommended.freshSession ? " (fresh session)" : "";
			ctx.ui.notify(
				`scramjet: next would be ${cleanForNotify(result.recommended.message)}${fresh}; edge setting: pause`,
				"info",
			);
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

	function routeCompleted(
		policy: NextStepPolicy,
		status: CommandStatusPayload,
		ctx: ExtensionContext,
		sourceName: string,
	) {
		if (policy.mode === "forced") {
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

		if (!autonomyValidated && state.registry.size > 0) {
			try {
				const config = loadAutonomyConfig(state.autonomyConfigPath);
				autonomyValidated = true;
				if (config) {
					const warnings = validateConfig(config, state.registry);
					for (const w of warnings) {
						ctx.ui.notify(`scramjet: autonomy.yaml: ${w}`, "warning");
					}
				}
			} catch (err) {
				autonomyValidated = false;
				ctx.ui.notify(`scramjet: ${(err as Error).message}; edge settings ignored until fixed`, "warning");
			}
		}

		const recommendedName = result.recommended?.parsedCommand?.name;
		let edgeSetting: EdgeSetting = null;
		if (recommendedName) {
			try {
				edgeSetting = resolveEdgeBehavior(state.autonomyConfigPath, sourceName, recommendedName);
			} catch (err) {
				ctx.ui.notify(`scramjet: ${(err as Error).message}; edge settings ignored this dispatch`, "warning");
			}
		}

		if (edgeSetting === "chain" && result.recommended?.parsedCommand) {
			executeStep(toDispatchStep(result.recommended, result.recommended.parsedCommand), ctx);
			return;
		}

		if (ctx.hasUI) {
			if (result.recommendedReason) {
				const level = state.enabled && result.valid.every((option) => !option.parsedCommand) ? "info" : "warning";
				ctx.ui.notify(
					`scramjet: ${result.recommendedReason}; valid option(s): ${optionsSummary(result.valid)}`,
					level,
				);
			}
			if (edgeSetting === "pause") {
				showSelector(result, ctx, { forcePause: true });
			} else {
				showSelector(result, ctx);
			}
		} else {
			routeWithoutUi(result, ctx, edgeSetting);
		}
	}

	function scheduleCompletedDispatch(
		policy: NextStepPolicy,
		status: CommandStatusPayload,
		ctx: ExtensionContext,
		sourceName: string,
	) {
		if (dispatchTimer) clearTimeout(dispatchTimer);
		// Deferred to land after the run settles; a throw here becomes an uncaughtException and leaves lifecycle wedged.
		dispatchTimer = setTimeout(() => {
			dispatchTimer = null;
			try {
				routeCompleted(policy, status, ctx, sourceName);
			} catch (err) {
				ctx.ui.notify(
					`scramjet: next-step dispatch failed (${(err as Error).message}); auto-continue paused`,
					"warning",
				);
			}
		}, 0);
	}

	function routeNonCompleted(status: CommandStatusPayload, ctx: ExtensionContext) {
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
				return;
		}
	}

	pi.on("agent_end", async (_event, ctx) => {
		const activeName = getActiveCommand(state.lifecycle);
		const def = activeName ? state.registry.get(activeName) : undefined;

		if (activeName && !def) {
			ctx.ui.notify(`scramjet: active command "${activeName}" not in registry; auto-continue skipped`, "warning");
			const exitResult = transition(state.lifecycle, { type: "workflow-exit" });
			if (exitResult.ok) applyTransition(state, exitResult);
			clearProbeWatchdog();
			return;
		}

		const policy = def?.next;

		switch (state.lifecycle.phase) {
			case "running": {
				if (!policy) {
					const exitResult = transition(state.lifecycle, { type: "workflow-exit" });
					if (exitResult.ok) applyTransition(state, exitResult);
					return;
				}
				const result = transition(state.lifecycle, { type: "agent-end" });
				if (!result.ok) return;
				applyTransition(state, result);
				scheduleProbe(policy, def.name);
				return;
			}
			case "probing": {
				clearProbeWatchdog();
				const result = transition(state.lifecycle, { type: "probe-self-healed" });
				if (result.ok) applyTransition(state, result);
				console.warn("scramjet: status probe turn ended without a valid status report; auto-continue paused");
				return;
			}
			case "reported": {
				clearProbeWatchdog();
				const lifecycle = state.lifecycle;
				if (lifecycle.phase !== "reported") return;
				const status = lifecycle.status;
				const command = lifecycle.command;

				if (status.status === "completed") {
					const termResult = transition(state.lifecycle, { type: "terminal-resolved", status: "completed" });
					if (termResult.ok) applyTransition(state, termResult);
					if (policy) scheduleCompletedDispatch(policy, status, ctx, command);
					return;
				}

				if (status.status === "waiting_for_user") {
					const waitResult = transition(state.lifecycle, { type: "waiting-parked" });
					if (waitResult.ok) applyTransition(state, waitResult);
				} else {
					const termResult = transition(state.lifecycle, {
						type: "terminal-resolved",
						status: status.status as "blocked" | "incomplete",
					});
					if (termResult.ok) applyTransition(state, termResult);
				}
				routeNonCompleted(status, ctx);
				return;
			}
			case "waiting":
			case "dormant":
			case "idle":
				return;
		}
	});

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
