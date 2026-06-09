import { describe, expect, it } from "vitest";
import { validateNextStep, validateNextSteps } from "../commands/validator.ts";
import type { CommandStatusCommandNextStep, CommandStatusNextStep } from "../types.ts";

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

describe("validateNextSteps — array form", () => {
	const closed = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	const entry = (over: Partial<CommandStatusCommandNextStep>): CommandStatusNextStep => ({
		name: "mach12:pr-review-fix",
		fresh_session: false,
		...over,
	});

	it("returns null and no skips for an empty array", () => {
		expect(validateNextSteps([], closed)).toEqual({ valid: null, skipped: [] });
	});

	it("returns null and no skips for undefined", () => {
		expect(validateNextSteps(undefined, closed)).toEqual({ valid: null, skipped: [] });
	});

	it("returns the first entry when it is valid and converts to a NextStep", () => {
		const result = validateNextSteps(
			[entry({ name: "mach12:pr-review-fix", args: "55", fresh_session: true, reason: "go" })],
			closed,
		);
		expect(result.valid).toEqual({ name: "mach12:pr-review-fix", args: "55", freshSession: true, reason: "go" });
		expect(result.skipped).toEqual([]);
	});

	it("skips invalid entries and returns the first valid one, recording the skipped names", () => {
		const result = validateNextSteps(
			[entry({ name: "z:not-in-list" }), entry({ name: "mach12:pr-pre-merge", fresh_session: false })],
			closed,
		);
		expect(result.valid).toEqual({
			name: "mach12:pr-pre-merge",
			args: undefined,
			freshSession: false,
			reason: undefined,
		});
		expect(result.skipped).toEqual(["z:not-in-list"]);
	});

	it("returns null with all names skipped and a reason when none are valid", () => {
		const result = validateNextSteps([entry({ name: "z:one" }), entry({ name: "z:two" })], closed);
		expect(result.valid).toBeNull();
		expect(result.skipped).toEqual(["z:one", "z:two"]);
		expect(result.reason).toContain("z:one");
	});

	it("under open mode, skips a blacklisted first entry and accepts the next, recording the skip", () => {
		const open = {
			mode: "open" as const,
			candidates: [{ name: "mach12:issue-review" }],
			blacklist: ["mach12:pr-merge"],
		};
		const result = validateNextSteps(
			[entry({ name: "mach12:pr-merge" }), entry({ name: "infra:rotate-key", fresh_session: false })],
			open,
		);
		expect(result.valid).toEqual({
			name: "infra:rotate-key",
			args: undefined,
			freshSession: false,
			reason: undefined,
		});
		expect(result.skipped).toEqual(["mach12:pr-merge"]);
	});

	it("rejects every entry under an ask policy", () => {
		const ask = { mode: "ask" as const, hint: "User picks" };
		const result = validateNextSteps([entry({ name: "x:y" }), entry({ name: "x:z" })], ask);
		expect(result.valid).toBeNull();
		expect(result.skipped).toEqual(["x:y", "x:z"]);
		expect(result.reason).toContain("ask");
	});
});
