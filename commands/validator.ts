import type { NextStepPolicy } from "../types.ts";

export interface ValidationResult {
	valid: boolean;
	reason?: string;
}

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
