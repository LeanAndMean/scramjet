import type { CommandStatusNextStep, NextStepPolicy } from "../types.ts";

// A next-step `message` that starts with "/" parses into a slash command: the
// bare command name plus an optional verbatim argument string. The harness
// owns this parse — the agent only ever supplies the message text, and the
// dispatch behavior (auto-dispatch vs. paste-to-editor) follows from whether
// the message parses as a command.
export interface ParsedSlashCommand {
	name: string;
	args?: string;
}

export function parseSlashCommand(message: string): ParsedSlashCommand | null {
	const trimmed = message.trim();
	if (!trimmed.startsWith("/")) return null;
	const spaceIdx = trimmed.indexOf(" ", 1);
	const name = spaceIdx === -1 ? trimmed.slice(1) : trimmed.slice(1, spaceIdx);
	if (!name) return null; // bare "/" or "/ foo" — not a valid command
	const args = spaceIdx === -1 ? undefined : trimmed.slice(spaceIdx + 1).trimStart() || undefined;
	return { name, args };
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

export interface ValidatedNextStep {
	index: number;
	reason: string;
	message: string;
	freshSession: boolean;
	// null = non-command message (pasted into the editor on selection).
	parsedCommand: ParsedSlashCommand | null;
}

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
	const parsedCommand = parseSlashCommand(step.message);
	if (!parsedCommand && policy.mode !== "open") {
		return { valid: false, reason: "non-command messages are valid only for open policies" };
	}
	if (parsedCommand) {
		const result = validateNextStep(parsedCommand.name, policy);
		if (!result.valid) return result;
	}
	if (!hasReason(step)) return { valid: false, reason: "selector-visible next steps must include reason" };
	return {
		valid: true,
		option: {
			index,
			reason: step.reason,
			message: step.message,
			freshSession: step.fresh_session ?? false,
			parsedCommand,
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
		skipped.push({ index, label: step.message.trim(), reason: result.reason });
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
