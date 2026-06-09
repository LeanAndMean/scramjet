import type { CommandStatusCommandNextStep, CommandStatusNextStep, NextStep, NextStepPolicy } from "../types.ts";

function isCommandStep(step: CommandStatusNextStep): step is CommandStatusCommandNextStep {
	return step.type === undefined || step.type === "command";
}

// Discriminated union: when valid is false a reason is required, so consumers
// can read `result.reason` after narrowing without optional-chaining.
export type ValidationResult = { valid: true } | { valid: false; reason: string };

type DecidedPolicy = Exclude<NextStepPolicy, { mode: "forced" }>;

export function validateNextStep(proposed: string | undefined, policy: DecidedPolicy): ValidationResult {
	switch (policy.mode) {
		case "closed":
			if (proposed === undefined) return { valid: true };
			if (policy.candidates.some((c) => c.name === proposed)) return { valid: true };
			return {
				valid: false,
				reason: `${proposed} is not in closed candidates [${policy.candidates.map((c) => c.name).join(", ")}]`,
			};
		case "open":
			if (proposed === undefined) return { valid: true };
			if (policy.blacklist?.includes(proposed)) {
				return { valid: false, reason: `${proposed} is blacklisted` };
			}
			return { valid: true };
		case "ask":
			if (proposed === undefined) return { valid: true };
			return { valid: false, reason: "ask mode pauses for the user; the agent must not pick" };
	}
}

// Result of validating a command-status `next_steps[]` array against a decided
// policy. `valid` is the first entry that passed (converted to a dispatchable
// NextStep) or null when none did; `skipped` lists the names rejected before
// the valid one (or every name when none passed); `reason` carries the first
// rejection reason, useful for the warning surfaced when nothing is valid.
export interface NextStepsValidation {
	valid: NextStep | null;
	skipped: string[];
	reason?: string;
}

// Array form of validateNextStep for the two-phase status protocol (issue 84).
// The agent supplies an ordered list of candidates; the MVP auto-continue
// dispatches the first one that passes the policy. Each entry is validated by
// name through the existing single-name validateNextStep, so closed/open/ask
// semantics stay identical. The array shape is forward-looking scaffolding for
// the choice-list UI; today only the first valid entry is acted on.
export function validateNextSteps(
	steps: readonly CommandStatusNextStep[] | undefined,
	policy: DecidedPolicy,
): NextStepsValidation {
	const skipped: string[] = [];
	let firstReason: string | undefined;
	for (const step of steps ?? []) {
		if (!isCommandStep(step)) {
			skipped.push(step.label ?? step.text);
			if (firstReason === undefined) firstReason = "free-text next steps are not dispatchable yet";
			continue;
		}
		const result = validateNextStep(step.name, policy);
		if (result.valid) {
			return {
				valid: {
					name: step.name,
					args: step.args,
					freshSession: step.fresh_session,
					reason: step.reason,
				},
				skipped,
			};
		}
		skipped.push(step.name);
		if (firstReason === undefined) firstReason = result.reason;
	}
	return { valid: null, skipped, reason: firstReason };
}
