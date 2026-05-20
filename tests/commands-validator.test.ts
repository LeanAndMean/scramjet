import { describe, expect, it } from "vitest";
import { validateNextStep } from "../commands/validator.ts";

describe("validateNextStep — closed mode", () => {
	const policy = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	it("accepts a candidate from the list", () => {
		expect(validateNextStep("mach12:pr-review-fix", policy)).toEqual({ valid: true });
	});

	it("rejects a pick outside the candidate list", () => {
		const result = validateNextStep("mach12:pr-merge", policy);
		if (result.valid) throw new Error("expected invalid");
		expect(result.reason).toContain("mach12:pr-merge");
		expect(result.reason).toContain("mach12:pr-review-fix");
	});

	it("accepts undefined (agent stops the chain)", () => {
		expect(validateNextStep(undefined, policy)).toEqual({ valid: true });
	});
});

describe("validateNextStep — open mode", () => {
	it("accepts any non-blacklisted command", () => {
		const policy = { mode: "open" as const, candidates: [{ name: "mach12:issue-review" }] };
		expect(validateNextStep("infra:rotate-key", policy)).toEqual({ valid: true });
	});

	it("accepts a listed candidate", () => {
		const policy = { mode: "open" as const, candidates: [{ name: "mach12:issue-review" }] };
		expect(validateNextStep("mach12:issue-review", policy)).toEqual({ valid: true });
	});

	it("rejects a blacklisted command", () => {
		const policy = {
			mode: "open" as const,
			candidates: [{ name: "mach12:issue-review" }],
			blacklist: ["mach12:pr-merge"],
		};
		const result = validateNextStep("mach12:pr-merge", policy);
		if (result.valid) throw new Error("expected invalid");
		expect(result.reason).toContain("blacklisted");
	});

	it("accepts undefined regardless of blacklist", () => {
		const policy = {
			mode: "open" as const,
			candidates: [{ name: "mach12:issue-review" }],
			blacklist: ["mach12:pr-merge"],
		};
		expect(validateNextStep(undefined, policy)).toEqual({ valid: true });
	});
});

describe("validateNextStep — ask mode", () => {
	const policy = { mode: "ask" as const, hint: "User picks" };

	it("accepts undefined (agent correctly defers to user)", () => {
		expect(validateNextStep(undefined, policy)).toEqual({ valid: true });
	});

	it("rejects any agent pick", () => {
		const result = validateNextStep("mach12:pr-merge", policy);
		if (result.valid) throw new Error("expected invalid");
		expect(result.reason).toContain("ask");
	});
});
