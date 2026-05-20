import type { NextStepPolicy } from "../types.ts";

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
