import type { ExtensionContext } from "@leanandmean/coding-agent";
import type { NextStep, ScramjetState } from "./types.js";

export type NextStepDispatchOrigin = "agent" | "forced";

export interface DispatchNextStepOptions {
	origin: NextStepDispatchOrigin;
}

export function buildNextStepWire(step: Pick<NextStep, "name" | "args">): string {
	// Reconstruct the slash-form wire payload from the structured NextStep.
	// The agent supplies bare `name` and optional `args`; Scramjet owns only
	// the slash prefix/join while Pi owns routing and prompt-template expansion.
	// S4: trimStart the args so a model-supplied leading space can't produce a
	// double space ("/cmd  84") — the schema asks for "no leading space", and we
	// already prepend the separator ourselves.
	const args = step.args?.trimStart();
	return `/${step.name}${args ? ` ${args}` : ""}`;
}

export function dispatchNextStep(
	ctx: ExtensionContext,
	state: ScramjetState,
	step: NextStep,
	options: DispatchNextStepOptions,
): void {
	const wire = buildNextStepWire(step);
	const forcedTarget = options.origin === "forced" ? step.name : null;
	if (forcedTarget) state.pendingForcedDispatch = forcedTarget;

	const clearPendingForced = () => {
		if (forcedTarget && state.pendingForcedDispatch === forcedTarget) state.pendingForcedDispatch = null;
	};

	const notifyFailure = (message: string, err?: unknown) => {
		clearPendingForced();
		const suffix = err instanceof Error ? ` (${err.message})` : err === undefined ? "" : ` (${String(err)})`;
		ctx.ui.notify(`scramjet: ${message}${suffix}`, "warning");
	};

	try {
		if (step.freshSession) {
			ctx.newSession({
				withSession: async (newCtx) => {
					await newCtx.dispatchUserInput(wire, { deliverAs: "followUp" });
				},
			})
				.then((result) => {
					if (result.cancelled) notifyFailure("fresh-session next-step dispatch cancelled");
				})
				.catch((err) => notifyFailure("fresh-session next-step dispatch failed", err));
			return;
		}

		ctx.dispatchUserInput(wire, { deliverAs: "followUp" }).catch((err) => {
			const prefix = options.origin === "forced" ? "forced dispatch failed" : "next-step dispatch failed";
			notifyFailure(prefix, err);
		});
	} catch (err) {
		const prefix = step.freshSession
			? "fresh-session next-step dispatch failed"
			: options.origin === "forced"
				? "forced dispatch failed"
				: "next-step dispatch failed";
		notifyFailure(prefix, err);
	}
}
