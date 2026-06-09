import type {
	CommandStatusCommandNextStep,
	CommandStatusFreeTextNextStep,
	CommandStatusNextStep,
	NextStep,
	NextStepPolicy,
} from "../types.ts";

function isCommandStep(step: CommandStatusNextStep): step is CommandStatusCommandNextStep {
	return step.type === undefined || step.type === "command";
}

function isFreeTextStep(step: CommandStatusNextStep): step is CommandStatusFreeTextNextStep {
	return step.type === "freetext";
}

function stepLabel(step: CommandStatusNextStep): string {
	if (isCommandStep(step)) return step.name;
	return step.label ?? step.text;
}

function hasReason(step: CommandStatusNextStep): step is CommandStatusNextStep & { reason: string } {
	return step.reason?.trim() !== "" && step.reason !== undefined;
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

export type ValidatedNextStep =
	| {
			type: "command";
			index: number;
			label?: string;
			reason: string;
			step: NextStep;
	  }
	| {
			type: "freetext";
			index: number;
			label?: string;
			reason: string;
			text: string;
	  };

export interface SkippedNextStep {
	index: number;
	label: string;
	reason: string;
}

export interface NextStepsValidation {
	valid: ValidatedNextStep[];
	skipped: SkippedNextStep[];
	recommended: ValidatedNextStep | null;
	recommendedReason?: string;
	reason?: string;
}

function validateDisplayableStep(
	step: CommandStatusNextStep,
	policy: DecidedPolicy,
	index: number,
): { valid: true; option: ValidatedNextStep } | { valid: false; reason: string } {
	if (isFreeTextStep(step)) {
		if (policy.mode !== "open") {
			return { valid: false, reason: "free-text next steps are valid only for open policies" };
		}
		if (!hasReason(step)) return { valid: false, reason: "selector-visible next steps must include reason" };
		return {
			valid: true,
			option: {
				type: "freetext",
				index,
				label: step.label,
				reason: step.reason,
				text: step.text,
			},
		};
	}

	const result = validateNextStep(step.name, policy);
	if (!result.valid) return result;
	if (!hasReason(step)) return { valid: false, reason: "selector-visible next steps must include reason" };
	return {
		valid: true,
		option: {
			type: "command",
			index,
			label: step.label,
			reason: step.reason,
			step: {
				name: step.name,
				args: step.args,
				freshSession: step.fresh_session,
				reason: step.reason,
			},
		},
	};
}

export function validateNextSteps(
	steps: readonly CommandStatusNextStep[] | undefined,
	policy: DecidedPolicy,
	recommendedIndex?: number,
): NextStepsValidation {
	const valid: ValidatedNextStep[] = [];
	const skipped: SkippedNextStep[] = [];
	let firstReason: string | undefined;

	for (const [index, step] of (steps ?? []).entries()) {
		const result = validateDisplayableStep(step, policy, index);
		if (result.valid) {
			valid.push(result.option);
			continue;
		}
		skipped.push({ index, label: stepLabel(step), reason: result.reason });
		if (firstReason === undefined) firstReason = result.reason;
	}

	let recommended: ValidatedNextStep | null = null;
	let recommendedReason: string | undefined;
	if (recommendedIndex !== undefined) {
		recommended = valid.find((option) => option.index === recommendedIndex) ?? null;
		if (!recommended) {
			const skippedRecommendation = skipped.find((step) => step.index === recommendedIndex);
			recommendedReason = skippedRecommendation
				? `recommended_next_step ${recommendedIndex} points to invalid next step ${skippedRecommendation.label}: ${skippedRecommendation.reason}`
				: `recommended_next_step ${recommendedIndex} is outside next_steps`;
		}
	} else if (valid.length > 0) {
		recommendedReason = "missing recommended_next_step; no automatic next-step dispatch";
	}

	return { valid, skipped, recommended, recommendedReason, reason: firstReason };
}
