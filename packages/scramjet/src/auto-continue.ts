/**
 * Auto-continuation, two-phase command-status protocol (issue 84).
 *
 * A top-level Scramjet command produces its normal user-facing answer in one
 * turn (the answer turn injects nothing about completion). On that turn's
 * agent_end, if the command declares a next-step policy, this driver begins the
 * probe and DEFERS a hidden status-check probe — a custom message that triggers
 * a short second turn in which the agent calls report_scramjet_command_status.
 * The tool records the status; this driver reads it on the probe turn's
 * agent_end and validates/dispatches/pauses.
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
 * Scramjet shows a selector and `/autopilot on` auto-selects a recommended
 * command after a 3s countdown; without UI it dispatches only a valid
 * recommended command under `/autopilot on`. Dispatch uses Pi's
 * dispatchUserInput so slash commands run through Pi's normal input pipeline.
 * See CLAUDE.md "MVP design rationales".
 */

import type { ExtensionAPI, ExtensionContext } from "@leanandmean/coding-agent";
import { loadAutonomyConfig, resolveEdgeBehavior, validateConfig } from "./autonomy-settings.js";
import { COMMAND_STATUS_PROBE_TYPE } from "./command-status.js";
import { parseSlashCommand, type ValidatedNextStep, validateNextSteps } from "./commands/validator.js";
import {
	activeCommandName,
	beginProbe,
	clearActiveCommand,
	enterDormant,
	hasTerminalReport,
	isDormant,
	isParkedForInput,
	isProbeDue,
	isProbeInFlight,
	derivePhaseLabel as lp,
} from "./lifecycle.js";
import { buildProbeMessage } from "./next-step.js";
import { dispatchNextStep } from "./next-step-dispatch.js";
import { selectNextStep } from "./next-step-selector.js";
import type {
	CommandStatusNextStep,
	CommandStatusRestingPayload,
	EdgeSetting,
	NextStep,
	NextStepPolicy,
	PendingSuggestion,
	ScramjetState,
} from "./types.js";

const COUNTDOWN_SECONDS = 3;
// Liveness watchdog window. Generous on purpose — a live probe turn is a
// single report_scramjet_command_status tool call and reports well within this,
// and the guard inside the timer re-checks the facts so a turn that DID
// complete (or already self-healed) is never clobbered. The value only bounds
// how long a probe that never produced a turn at all (dropped triggerTurn
// during run settle, Escape before the turn starts, session teardown mid-turn)
// lingers at probeInFlight before self-healing; the next real command resets
// the lifecycle anyway. Kept comfortably longer than any plausible probe turn so
// it cannot fire while the model is still thinking before its tool call (probe
// still in flight), which would otherwise drop a legitimate chain — worse than
// the stall it fixes.
const PROBE_WATCHDOG_MS = 30_000;

// Parse a raw report entry's message into a dispatchable NextStep, or null
// when the message is not a slash command. Used for the forced-handoff path,
// which reads the raw payload before selector validation.
function toNextStep(step: CommandStatusNextStep | undefined): NextStep | undefined {
	if (!step) return undefined;
	const parsed = parseSlashCommand(step.message);
	if (!parsed) return undefined;
	return { name: parsed.name, args: parsed.args, freshSession: step.fresh_session, reason: step.reason };
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
// routeNonCompleted's blocked notify.
export const NOTIFY_MAX = 200;
export function cleanForNotify(text: string): string {
	const collapsed = text
		.replace(/[\u0000-\u001f\u007f]/g, " ")
		.trim()
		.replace(/\s+/g, " ");
	return collapsed.length > NOTIFY_MAX ? `${collapsed.slice(0, NOTIFY_MAX - 1)}…` : collapsed;
}

// Extract stopReason from the last assistant message in an agent_end event.
export function extractStopReason(event: { messages?: unknown[] }): string | undefined {
	if (!Array.isArray(event.messages)) return undefined;
	for (let i = event.messages.length - 1; i >= 0; i--) {
		const msg = event.messages[i] as { role?: string; stopReason?: string } | undefined;
		if (msg?.role === "assistant") return msg.stopReason;
	}
	return undefined;
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

	function clearProbeTimer(reason = "cleared") {
		if (probeTimer) {
			clearTimeout(probeTimer);
			probeTimer = null;
			const command = activeCommandName(state.lifecycle);
			state.logger.lifecycle("status probe timer cleared", {
				phase: lp(state.lifecycle),
				...(command ? { command } : {}),
				detail: { reason },
			});
		}
	}

	function clearProbeWatchdog(reason = "cleared") {
		if (probeWatchdog) {
			clearTimeout(probeWatchdog);
			probeWatchdog = null;
			const command = activeCommandName(state.lifecycle);
			state.logger.lifecycle("probe watchdog cleared", {
				phase: lp(state.lifecycle),
				...(command ? { command } : {}),
				detail: { reason },
			});
		}
	}

	function armProbeWatchdog() {
		clearProbeWatchdog("rearm");
		if (isProbeInFlight(state.lifecycle)) {
			const command = state.lifecycle.activeCommand!;
			const watchdogGeneration = state.lifecycleGeneration;
			probeWatchdog = setTimeout(() => {
				probeWatchdog = null;
				if (state.lifecycleGeneration !== watchdogGeneration || activeCommandName(state.lifecycle) !== command) {
					state.logger.lifecycle("probe watchdog stale", {
						phase: lp(state.lifecycle),
						command,
						detail: { scheduledGeneration: watchdogGeneration, currentGeneration: state.lifecycleGeneration },
					});
					return;
				}
				if (isProbeInFlight(state.lifecycle)) {
					state.logger.lifecycle("probe watchdog fired", {
						phase: lp(state.lifecycle),
						command,
						detail: { timeoutMs: PROBE_WATCHDOG_MS, generation: watchdogGeneration },
					});
					enterDormant(state, "watchdog-timeout");
					state.logger.warn("probe", "status probe turn never completed; auto-continue paused", {
						phase: lp(state.lifecycle),
					});
				}
			}, PROBE_WATCHDOG_MS);
			state.logger.lifecycle("probe watchdog armed", {
				phase: lp(state.lifecycle),
				command,
				detail: { timeoutMs: PROBE_WATCHDOG_MS, generation: watchdogGeneration },
			});
		}
	}

	state.suspendProbeWatchdog = () => clearProbeWatchdog("suspended");
	state.rearmProbeWatchdog = armProbeWatchdog;
	state.clearLifecycleTimers = (reason = "lifecycle-reset") => {
		cancelSelector();
		clearProbeTimer(reason);
		clearProbeWatchdog(reason);
		clearDispatchTimer(reason);
	};

	function clearDispatchTimer(reason = "cleared") {
		if (dispatchTimer) {
			clearTimeout(dispatchTimer);
			dispatchTimer = null;
			const command = activeCommandName(state.lifecycle);
			state.logger.lifecycle("completed dispatch timer cleared", {
				phase: lp(state.lifecycle),
				...(command ? { command } : {}),
				detail: { reason },
			});
		}
	}

	function executeStep(step: NextStep, ctx: ExtensionContext) {
		state.logger.lifecycle("next step dispatching", {
			phase: lp(state.lifecycle),
			command: step.name,
			detail: { args: step.args, freshSession: step.freshSession, reason: step.reason },
		});
		dispatchNextStep(ctx, state, step, { origin: "agent" });
	}

	// Build the dispatchable NextStep for a validated option whose message
	// parsed as a slash command.
	function toDispatchStep(option: ValidatedNextStep, parsed: NonNullable<ValidatedNextStep["parsedCommand"]>) {
		return { name: parsed.name, args: parsed.args, freshSession: option.freshSession, reason: option.reason };
	}

	function delegateOnlyCheck(name: string): string | null {
		const def = state.registry.get(name);
		if (def?.delegateOnly) return `${name} is delegate-only (invoke via delegate, not top-level dispatch)`;
		return null;
	}

	function dispatchForced(target: string, handoff: NextStep | undefined, ctx: ExtensionContext): boolean {
		const command = activeCommandName(state.lifecycle);
		const def = state.registry.get(target);
		if (!def) {
			state.logger.lifecycle("forced dispatch skipped", {
				phase: lp(state.lifecycle),
				...(command ? { command } : {}),
				detail: { target, reason: "target-not-in-registry" },
			});
			ctx.ui.notify(`scramjet: forced target "${target}" not in registry; auto-continue skipped`, "warning");
			return false;
		}
		if (def.delegateOnly) {
			state.logger.lifecycle("forced dispatch skipped", {
				phase: lp(state.lifecycle),
				...(command ? { command } : {}),
				detail: { target, reason: "target-is-delegate-only" },
			});
			ctx.ui.notify(`scramjet: forced target "${target}" is delegate-only; auto-continue skipped`, "warning");
			return false;
		}

		let step: NextStep = { name: target, freshSession: false };
		if (handoff) {
			if (handoff.name === target) {
				step = { ...handoff, name: target };
			} else {
				state.logger.lifecycle("forced handoff ignored", {
					phase: lp(state.lifecycle),
					...(command ? { command } : {}),
					detail: { target, supplied: handoff.name },
				});
				ctx.ui.notify(
					`scramjet: forced target is "${target}"; agent supplied next_steps name "${handoff.name}" — ignoring supplied forced handoff`,
					"warning",
				);
			}
		}

		state.logger.lifecycle("next step dispatching", {
			phase: lp(state.lifecycle),
			command: step.name,
			detail: { origin: "forced", args: step.args, freshSession: step.freshSession, reason: step.reason },
		});
		dispatchNextStep(ctx, state, step, { origin: "forced" });
		return true;
	}

	// Schedule the hidden status-check probe on a deferred tick so it lands after
	// the run is idle (see file header). triggerTurn starts the short probe turn;
	// display:false keeps the message out of the TUI while it still persists in
	// the journal and reaches the model as user context.
	function scheduleProbe(policy: NextStepPolicy | undefined, commandId: string) {
		if (probeTimer) {
			clearTimeout(probeTimer);
			state.logger.lifecycle("status probe timer cleared", {
				phase: lp(state.lifecycle),
				command: commandId,
				detail: { reason: "reschedule" },
			});
		}
		clearProbeWatchdog("probe-rescheduled");
		const content = buildProbeMessage(policy, commandId, state.enabled);
		const probeGeneration = state.lifecycleGeneration;
		state.logger.lifecycle("status probe scheduled", {
			phase: lp(state.lifecycle),
			command: commandId,
			detail: { policyMode: policy?.mode ?? "none", enabled: state.enabled, generation: probeGeneration },
		});
		probeTimer = setTimeout(() => {
			probeTimer = null;
			if (state.lifecycleGeneration !== probeGeneration || activeCommandName(state.lifecycle) !== commandId) {
				state.logger.lifecycle("status probe timer stale", {
					phase: lp(state.lifecycle),
					command: commandId,
					detail: { scheduledGeneration: probeGeneration, currentGeneration: state.lifecycleGeneration },
				});
				return;
			}
			state.logger.lifecycle("status probe timer fired", {
				phase: lp(state.lifecycle),
				command: commandId,
			});
			try {
				pi.sendMessage({ customType: COMMAND_STATUS_PROBE_TYPE, content, display: false }, { triggerTurn: true });
				state.logger.lifecycle("status probe sent", {
					phase: lp(state.lifecycle),
					command: commandId,
					detail: { triggerTurn: true },
				});
				armProbeWatchdog();
			} catch (err) {
				const message = (err as Error).message;
				enterDormant(state, `send-failure: ${message}`);
				state.logger.warn("probe", `status probe failed to send (${message}); auto-continue paused`, {
					error: message,
				});
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
			state.logger.lifecycle("next step pasted", {
				phase: lp(state.lifecycle),
				detail: { message: option.message, reason: option.reason },
			});
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
		{ forcePause = false, title }: { forcePause?: boolean; title?: string } = {},
	) {
		cancelSelector();
		const selectorId = activeSelectorId;
		const selectorGeneration = state.lifecycleGeneration;
		const selectorCommand = activeCommandName(state.lifecycle);
		const controller = new AbortController();
		activeSelectorAbort = controller;
		const autoSelect =
			!forcePause && state.enabled && result.recommended?.parsedCommand ? result.recommended : undefined;
		state.logger.lifecycle("next-step selector shown", {
			phase: lp(state.lifecycle),
			detail: {
				validCount: result.valid.length,
				recommended: result.recommended?.message,
				autoSelect: autoSelect?.message,
				forcePause,
			},
		});
		const scoped = ctx.scopedModels;
		const models =
			scoped.length > 0
				? scoped.filter((s) => ctx.modelRegistry.hasConfiguredAuth(s.model)).map((s) => s.model)
				: ctx.modelRegistry.getAvailable();
		const initialModel = ctx.model;
		void selectNextStep(ctx, {
			title,
			options: result.valid,
			recommended: result.recommended,
			autoSelect,
			countdownSeconds: autoSelect ? COUNTDOWN_SECONDS : 0,
			signal: controller.signal,
			models,
			initialModel,
		})
			.then(async (selection) => {
				if (
					selectorId !== activeSelectorId ||
					state.lifecycleGeneration !== selectorGeneration ||
					activeCommandName(state.lifecycle) !== selectorCommand
				) {
					return;
				}
				activeSelectorAbort = null;
				if (!selection) {
					state.logger.lifecycle("next-step selector closed", {
						phase: lp(state.lifecycle),
						detail: { reason: "no-selection" },
					});
					return;
				}

				if (selection.model) {
					try {
						// pi.setModel before dispatch — ordering matters for issue 186 model snapshot
						const ok = await pi.setModel(selection.model);
						if (!ok) {
							ctx.ui.notify(
								`scramjet: model switch to ${selection.model.name} failed (no API key); dispatching with current model`,
								"warning",
							);
						}
					} catch (err) {
						ctx.ui.notify(
							`scramjet: model switch to ${selection.model.name} failed (${selectorErrorMessage(err)}); dispatching with current model`,
							"warning",
						);
					}
					// Re-run stale guard after awaiting pi.setModel
					if (
						selectorId !== activeSelectorId ||
						state.lifecycleGeneration !== selectorGeneration ||
						activeCommandName(state.lifecycle) !== selectorCommand
					) {
						return;
					}
				}

				runSelectedOption(selection.step, ctx);
			})
			.catch((err) => {
				if (
					selectorId !== activeSelectorId ||
					state.lifecycleGeneration !== selectorGeneration ||
					activeCommandName(state.lifecycle) !== selectorCommand
				) {
					if (!isExpectedSelectorCancellation(err)) {
						const message = selectorErrorMessage(err);
						state.logger.warn("dispatch", `stale next-step selector failed (${message}); failure ignored`, {
							error: message,
						});
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
		sourceName?: string,
	) {
		if (!result.recommended) {
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				...(sourceName ? { command: sourceName } : {}),
				detail: { reason: result.recommendedReason ?? "no-recommended-option", validCount: result.valid.length },
			});
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
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				...(sourceName ? { command: sourceName } : {}),
				detail: { reason: "recommended-not-command", message: text, enabled: state.enabled },
			});
			if (state.enabled) {
				ctx.ui.notify(
					`scramjet: recommended next step is not a slash command (${text}); automatic dispatch skipped`,
					"warning",
				);
			} else {
				ctx.ui.notify(
					`scramjet: next would be ${text}; /autopilot on only auto-dispatches command next steps`,
					"info",
				);
			}
			return;
		}

		if (edgeSetting === "pause") {
			const fresh = result.recommended.freshSession ? " (fresh session)" : "";
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				...(sourceName ? { command: sourceName } : {}),
				detail: { reason: "edge-paused", message: result.recommended.message },
			});
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
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				...(sourceName ? { command: sourceName } : {}),
				detail: { reason: "scramjet-disabled", message: result.recommended.message },
			});
			ctx.ui.notify(
				`scramjet: next would be ${cleanForNotify(result.recommended.message)}${fresh}; /autopilot on to chain`,
				"info",
			);
		}
	}

	function routeCompleted(
		policy: NextStepPolicy,
		status: CommandStatusRestingPayload,
		ctx: ExtensionContext,
		sourceName: string,
	) {
		state.logger.lifecycle("next-step policy evaluated", {
			phase: lp(state.lifecycle),
			command: sourceName,
			detail: {
				mode: policy.mode,
				status: status.status,
				nextStepCount: status.next_steps?.length ?? 0,
				recommendedNextStep: status.recommended_next_step,
				enabled: state.enabled,
				hasUI: ctx.hasUI,
			},
		});
		if (policy.mode === "forced") {
			const handoff = toNextStep(status.next_steps?.[0]);
			dispatchForced(policy.target, handoff, ctx);
			return;
		}

		if (policy.mode === "ask") {
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				command: sourceName,
				detail: { reason: "ask-mode", proposedCount: status.next_steps?.length ?? 0 },
			});
			if (status.next_steps?.length) {
				ctx.ui.notify(
					"scramjet: ask-mode command; agent proposed next steps — ignored, waiting for user",
					"warning",
				);
			}
			return;
		}

		const result = validateNextSteps(status.next_steps, policy, status.recommended_next_step, delegateOnlyCheck);
		if (!result.valid.length) {
			state.logger.lifecycle("next-step dispatch skipped", {
				phase: lp(state.lifecycle),
				command: sourceName,
				detail: { reason: result.reason ?? "no-valid-options" },
			});
			if (result.reason) ctx.ui.notify(`scramjet: ${result.reason}`, "warning");
			return;
		}

		if (result.skipped.length) {
			state.logger.lifecycle("next-step options skipped", {
				phase: lp(state.lifecycle),
				command: sourceName,
				detail: { skippedCount: result.skipped.length, validCount: result.valid.length },
			});
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
			state.logger.lifecycle("next-step edge setting applied", {
				phase: lp(state.lifecycle),
				command: sourceName,
				detail: { edgeSetting, target: result.recommended.parsedCommand.name },
			});
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
				state.logger.lifecycle("next-step edge setting applied", {
					phase: lp(state.lifecycle),
					command: sourceName,
					detail: { edgeSetting },
				});
				showSelector(result, ctx, { forcePause: true });
			} else {
				showSelector(result, ctx);
			}
		} else {
			routeWithoutUi(result, ctx, edgeSetting, sourceName);
		}
	}

	function scheduleCompletedDispatch(
		policy: NextStepPolicy,
		status: CommandStatusRestingPayload,
		ctx: ExtensionContext,
		sourceName: string,
	) {
		clearDispatchTimer("reschedule");
		const dispatchGeneration = state.lifecycleGeneration;
		state.logger.lifecycle("completed dispatch scheduled", {
			phase: lp(state.lifecycle),
			command: sourceName,
			detail: { policyMode: policy.mode, status: status.status, generation: dispatchGeneration },
		});
		dispatchTimer = setTimeout(() => {
			dispatchTimer = null;
			if (state.lifecycleGeneration !== dispatchGeneration || activeCommandName(state.lifecycle) !== null) {
				state.logger.lifecycle("completed dispatch timer stale", {
					phase: lp(state.lifecycle),
					command: sourceName,
					detail: {
						scheduledGeneration: dispatchGeneration,
						currentGeneration: state.lifecycleGeneration,
						activeCommand: activeCommandName(state.lifecycle),
					},
				});
				return;
			}
			state.logger.lifecycle("completed dispatch timer fired", {
				phase: lp(state.lifecycle),
				command: sourceName,
				detail: { policyMode: policy.mode, status: status.status },
			});
			try {
				routeCompleted(policy, status, ctx, sourceName);
			} catch (err) {
				state.logger.lifecycle("completed dispatch failed", {
					phase: lp(state.lifecycle),
					command: sourceName,
					detail: { error: (err as Error).message },
				});
				ctx.ui.notify(
					`scramjet: next-step dispatch failed (${(err as Error).message}); auto-continue paused`,
					"warning",
				);
			}
		}, 0);
	}

	function scheduleSuggestionDispatch(suggestion: PendingSuggestion, ctx: ExtensionContext) {
		clearDispatchTimer("suggestion-reschedule");
		const dispatchGeneration = state.lifecycleGeneration;
		// Object identity is load-bearing: generation alone cannot detect a same-generation
		// re-store (last-write-wins within one run), so we check both.
		const capturedSuggestion = suggestion;
		state.logger.lifecycle("suggestion dispatch scheduled", {
			phase: lp(state.lifecycle),
			detail: {
				stepCount: suggestion.steps.length,
				recommendedIndex: suggestion.recommendedIndex,
				generation: dispatchGeneration,
			},
		});
		dispatchTimer = setTimeout(() => {
			dispatchTimer = null;
			if (
				state.lifecycleGeneration !== dispatchGeneration ||
				activeCommandName(state.lifecycle) !== null ||
				state.pendingSuggestion !== capturedSuggestion
			) {
				state.logger.lifecycle("suggestion dispatch timer stale", {
					phase: lp(state.lifecycle),
					detail: {
						scheduledGeneration: dispatchGeneration,
						currentGeneration: state.lifecycleGeneration,
						activeCommand: activeCommandName(state.lifecycle),
						identityMatch: state.pendingSuggestion === capturedSuggestion,
					},
				});
				return;
			}
			// Consume and clear
			const consumed = state.pendingSuggestion;
			state.pendingSuggestion = null;
			if (!consumed) return;

			if (!ctx.hasUI) {
				state.logger.lifecycle("suggestion dispatch dropped", {
					phase: lp(state.lifecycle),
					detail: { reason: "no-ui" },
				});
				return;
			}

			// Re-validate against current registry (may have been reassigned)
			const openPolicy = { mode: "open" as const, candidates: [] };
			const result = validateNextSteps(consumed.steps, openPolicy, consumed.recommendedIndex, delegateOnlyCheck);
			if (!result.valid.length) {
				state.logger.lifecycle("suggestion dispatch dropped", {
					phase: lp(state.lifecycle),
					detail: { reason: result.reason ?? "no-valid-options" },
				});
				return;
			}

			state.logger.lifecycle("suggestion dispatch fired", {
				phase: lp(state.lifecycle),
				detail: {
					validCount: result.valid.length,
					recommended: result.recommended?.message,
				},
			});

			try {
				showSelector(result, ctx, { forcePause: true, title: "Agent suggests a next step" });
			} catch (err) {
				state.logger.lifecycle("suggestion dispatch failed", {
					phase: lp(state.lifecycle),
					detail: { error: (err as Error).message },
				});
				ctx.ui.notify(
					`scramjet: suggestion selector failed (${(err as Error).message}); suggestion dropped`,
					"warning",
				);
			}
		}, 0);
	}

	function routeNonCompleted(status: CommandStatusRestingPayload, ctx: ExtensionContext) {
		switch (status.status) {
			case "blocked":
				ctx.ui.notify(`scramjet: command blocked — ${cleanForNotify(status.summary)}`, "warning");
				return;
			default:
				return;
		}
	}

	pi.on("agent_end", async (event, ctx) => {
		const activeName = activeCommandName(state.lifecycle);
		const def = activeName ? state.registry.get(activeName) : undefined;
		const stopReason = extractStopReason(event);
		state.logger.lifecycle("agent_end observed", {
			phase: lp(state.lifecycle),
			...(activeName ? { command: activeName } : {}),
			detail: {
				stopReason,
				hasPolicy: Boolean(def?.next),
				probeScheduled: probeTimer !== null,
				watchdogActive: probeWatchdog !== null,
				dispatchScheduled: dispatchTimer !== null,
			},
		});

		// Abort: user cancelled — enter dormant, clear all timers
		if (stopReason === "aborted" && activeName) {
			state.logger.lifecycle("agent_end abort", {
				phase: lp(state.lifecycle),
				command: activeName,
			});
			clearProbeTimer("aborted");
			clearProbeWatchdog("aborted");
			clearDispatchTimer("aborted");
			enterDormant(state, "aborted");
			return;
		}

		// Error: keep probe state valid for Pi retry safety. The watchdog self-heals
		// abandoned probes if no retried report arrives.
		if (stopReason === "error" && activeName) {
			if (isProbeInFlight(state.lifecycle)) {
				state.logger.lifecycle("probe error observed", {
					phase: lp(state.lifecycle),
					command: activeName,
					detail: { watchdogActive: probeWatchdog !== null },
				});
			}
			return;
		}

		if (activeName && !def) {
			state.logger.lifecycle("agent_end skipped", {
				phase: lp(state.lifecycle),
				command: activeName,
				detail: { reason: "active-command-not-in-registry" },
			});
			ctx.ui.notify(`scramjet: active command "${activeName}" not in registry; auto-continue skipped`, "warning");
			clearActiveCommand(state, "active-command-not-in-registry");
			state.clearLifecycleTimers?.("active-command-missing");
			return;
		}

		const policy = def?.next;

		// Probe-armed (agent finished a work turn; probe is due)
		if (isProbeDue(state.lifecycle)) {
			clearDispatchTimer("new-probe-cycle");
			state.logger.lifecycle("status probe preparing", {
				phase: lp(state.lifecycle),
				command: state.lifecycle.activeCommand,
				detail: { policyMode: policy?.mode ?? "none" },
			});
			const result = beginProbe(state, "agent-end");
			if (!result.ok) {
				state.logger.lifecycle("status probe skipped", {
					phase: lp(state.lifecycle),
					command: state.lifecycle.activeCommand,
					detail: { reason: "beginProbe-failed", error: result.reason },
				});
				return;
			}
			scheduleProbe(policy, def!.name);
			return;
		}

		// Probe in flight but agent_end arrived without a status report
		if (isProbeInFlight(state.lifecycle)) {
			clearProbeWatchdog("probe-turn-ended");
			enterDormant(state, "agent-end-without-status");
			state.logger.warn("probe", "status probe turn ended without a valid status report; auto-continue paused", {
				phase: lp(state.lifecycle),
			});
			return;
		}

		// Terminal report pending — route by status
		if (hasTerminalReport(state.lifecycle)) {
			clearProbeWatchdog("status-reported");
			const report = state.lifecycle.lastReport!;
			const command = state.lifecycle.activeCommand!;

			if (report.status === "completed") {
				clearActiveCommand(state, "completed");
				if (policy) {
					scheduleCompletedDispatch(policy, report, ctx, command);
				} else {
					state.logger.lifecycle("next-step dispatch skipped", {
						phase: lp(state.lifecycle),
						command,
						detail: { reason: "no-next-policy-after-report" },
					});
				}
				return;
			}

			// blocked / incomplete → dormant (command stays associated)
			enterDormant(state, report.status);
			routeNonCompleted(report, ctx);
			return;
		}

		// Parked for input or dormant — no action
		if (isParkedForInput(state.lifecycle) || isDormant(state.lifecycle)) {
			state.logger.lifecycle("agent_end skipped", {
				phase: lp(state.lifecycle),
				...(activeName ? { command: activeName } : {}),
				detail: { reason: "inactive-lifecycle" },
			});
			return;
		}

		// Idle with a pending suggestion — schedule the deferred selector
		if (activeName === null && state.pendingSuggestion) {
			// Own stopReason filtering (existing aborted/error guards are activeName-gated)
			if (stopReason === "aborted") {
				state.pendingSuggestion = null;
				state.logger.lifecycle("suggestion dropped", {
					phase: lp(state.lifecycle),
					detail: { reason: "aborted" },
				});
				return;
			}
			if (stopReason === "error") {
				state.logger.lifecycle("suggestion retained", {
					phase: lp(state.lifecycle),
					detail: { reason: "error-retry" },
				});
				return;
			}
			if (state.pendingSuggestion.generation !== state.lifecycleGeneration) {
				state.logger.lifecycle("suggestion dropped", {
					phase: lp(state.lifecycle),
					detail: {
						reason: "stale-generation",
						storedGeneration: state.pendingSuggestion.generation,
						currentGeneration: state.lifecycleGeneration,
					},
				});
				state.pendingSuggestion = null;
				return;
			}
			if (!ctx.hasUI) {
				state.logger.lifecycle("suggestion dropped", {
					phase: lp(state.lifecycle),
					detail: { reason: "no-ui" },
				});
				state.pendingSuggestion = null;
				return;
			}
			if (state.freetextAwaitingReply) {
				state.logger.lifecycle("suggestion dropped", {
					phase: lp(state.lifecycle),
					detail: { reason: "freetext-awaiting-reply" },
				});
				state.pendingSuggestion = null;
				return;
			}
			scheduleSuggestionDispatch(state.pendingSuggestion, ctx);
			return;
		}

		// Idle with no suggestion — no action
		if (activeName === null) {
			state.logger.lifecycle("agent_end skipped", {
				phase: lp(state.lifecycle),
				detail: { reason: "inactive-lifecycle" },
			});
			return;
		}
	});

	pi.on("session_compact", async () => {
		const wasProbing = isProbeInFlight(state.lifecycle);
		state.clearLifecycleTimers?.("session-compact");
		if (wasProbing && isProbeInFlight(state.lifecycle)) enterDormant(state, "session-compact");
		state.pendingSuggestion = null;
		state.freetextAwaitingReply = false;
	});

	pi.on("session_shutdown", async () => {
		state.clearLifecycleTimers?.("session-shutdown");
		state.pendingSuggestion = null;
		state.freetextAwaitingReply = false;
	});
}
