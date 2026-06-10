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

describe("validateNextStep — same-name-different-args (closed)", () => {
	const policy = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	it("accepts the same command name regardless of args", () => {
		expect(validateNextStep("mach12:pr-review-fix", policy)).toEqual({ valid: true });
		expect(validateNextStep("mach12:pr-review-fix", policy)).toEqual({ valid: true });
	});
});

describe("validateNextSteps — same-name-different-args entries", () => {
	const closed = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	const genuineOnly: CommandStatusCommandNextStep = {
		name: "mach12:pr-review-fix",
		args: "94 --review-comment 123 --assessment-comment 456",
		fresh_session: true,
		reason: "Address genuine issues only",
	};

	const withNitpicks: CommandStatusCommandNextStep = {
		name: "mach12:pr-review-fix",
		args: "94 --review-comment 123 --assessment-comment 456 --include-nitpicks",
		fresh_session: true,
		reason: "Address genuine issues and nitpicks",
	};

	const preMerge: CommandStatusCommandNextStep = {
		name: "mach12:pr-pre-merge",
		fresh_session: false,
		reason: "No issues found, proceed to merge",
	};

	it("accepts both same-name entries with different args under closed validation", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks], closed, 0);
		expect(result.valid).toHaveLength(2);
		expect(result.skipped).toEqual([]);
	});

	it("assigns distinct indexes to same-name entries", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 0);
		expect(result.valid.map((o) => o.index)).toEqual([0, 1, 2]);
	});

	it("preserves distinct args on each validated entry", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks], closed, 0);
		const steps = result.valid.filter((o) => o.type === "command").map((o) => o.step);
		expect(steps[0].args).toBe("94 --review-comment 123 --assessment-comment 456");
		expect(steps[1].args).toBe("94 --review-comment 123 --assessment-comment 456 --include-nitpicks");
	});

	it("recommendation correctly targets the second same-name entry", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 1);
		expect(result.recommended).toMatchObject({
			type: "command",
			index: 1,
			reason: "Address genuine issues and nitpicks",
			step: { name: "mach12:pr-review-fix", args: withNitpicks.args },
		});
	});

	it("recommendation correctly targets the first same-name entry", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 0);
		expect(result.recommended).toMatchObject({
			type: "command",
			index: 0,
			reason: "Address genuine issues only",
			step: { name: "mach12:pr-review-fix", args: genuineOnly.args },
		});
	});

	it("same-name entries with different args pass open validation", () => {
		const open = { mode: "open" as const, candidates: [{ name: "mach12:pr-review-fix" }] };
		const result = validateNextSteps([genuineOnly, withNitpicks], open, 0);
		expect(result.valid).toHaveLength(2);
		expect(result.skipped).toEqual([]);
	});
});

describe("validateNextSteps — selector-visible array form", () => {
	const closed = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	const entry = (over: Partial<CommandStatusCommandNextStep>): CommandStatusNextStep => ({
		name: "mach12:pr-review-fix",
		fresh_session: false,
		reason: "fits this workflow",
		...over,
	});

	it("returns empty validation for an empty array", () => {
		expect(validateNextSteps([], closed)).toEqual({
			valid: [],
			skipped: [],
			recommended: null,
			recommendedReason: undefined,
			reason: undefined,
		});
	});

	it("returns empty validation for undefined", () => {
		expect(validateNextSteps(undefined, closed)).toEqual({
			valid: [],
			skipped: [],
			recommended: null,
			recommendedReason: undefined,
			reason: undefined,
		});
	});

	it("accepts closed candidates and selects the recommendation by zero-based original index", () => {
		const result = validateNextSteps(
			[
				entry({ name: "mach12:pr-review-fix", args: "55", fresh_session: true, reason: "fix review findings" }),
				entry({ name: "mach12:pr-pre-merge", fresh_session: false, reason: "review is complete" }),
			],
			closed,
			1,
		);

		expect(result.valid).toHaveLength(2);
		expect(result.recommended).toEqual({
			type: "command",
			index: 1,
			label: undefined,
			reason: "review is complete",
			step: {
				name: "mach12:pr-pre-merge",
				args: undefined,
				freshSession: false,
				reason: "review is complete",
			},
		});
		expect(result.skipped).toEqual([]);
	});

	it("treats missing type as a legacy command entry", () => {
		const result = validateNextSteps([entry({ type: undefined, name: "mach12:pr-review-fix" })], closed, 0);
		expect(result.recommended?.type).toBe("command");
		expect(result.recommended).toMatchObject({ index: 0, step: { name: "mach12:pr-review-fix" } });
	});

	it("records skipped invalid entries with reasons without changing recommendation indexes", () => {
		const result = validateNextSteps(
			[entry({ name: "z:not-in-list" }), entry({ name: "mach12:pr-pre-merge", fresh_session: false })],
			closed,
			1,
		);

		expect(result.valid.map((option) => option.index)).toEqual([1]);
		expect(result.recommended).toMatchObject({ type: "command", index: 1, step: { name: "mach12:pr-pre-merge" } });
		expect(result.skipped).toEqual([
			{
				index: 0,
				label: "z:not-in-list",
				reason: expect.stringContaining("not in closed candidates"),
			},
		]);
	});

	it("rejects every invalid entry and keeps the first rejection reason", () => {
		const result = validateNextSteps([entry({ name: "z:one" }), entry({ name: "z:two" })], closed, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped.map((step) => step.label)).toEqual(["z:one", "z:two"]);
		expect(result.reason).toContain("z:one");
		expect(result.recommendedReason).toContain("recommended_next_step 0 points to invalid next step z:one");
	});

	it("accepts open command and free-text candidates", () => {
		const open = { mode: "open" as const, candidates: [{ name: "mach12:issue-review" }] };
		const result = validateNextSteps(
			[
				entry({ name: "infra:rotate-key", reason: "outside Scramjet but useful" }),
				{
					type: "freetext",
					text: "Please summarize the issue.",
					label: "Ask for summary",
					reason: "needs more context",
				},
			],
			open,
			1,
		);

		expect(result.valid).toHaveLength(2);
		expect(result.recommended).toEqual({
			type: "freetext",
			index: 1,
			label: "Ask for summary",
			reason: "needs more context",
			text: "Please summarize the issue.",
		});
	});

	it("rejects blacklisted open command candidates", () => {
		const open = {
			mode: "open" as const,
			candidates: [{ name: "mach12:issue-review" }],
			blacklist: ["mach12:pr-merge"],
		};
		const result = validateNextSteps([entry({ name: "mach12:pr-merge" })], open, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0]).toMatchObject({
			label: "mach12:pr-merge",
			reason: expect.stringContaining("blacklisted"),
		});
	});

	it("rejects free-text outside open policies", () => {
		const result = validateNextSteps(
			[{ type: "freetext", text: "Continue in prose", reason: "not a command" }],
			closed,
			0,
		);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0].reason).toContain("open policies");
	});

	it("rejects missing reason for selector-visible command entries", () => {
		const result = validateNextSteps([entry({ reason: undefined })], closed, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0].reason).toContain("must include reason");
	});

	it("rejects missing reason for selector-visible free-text entries", () => {
		const open = { mode: "open" as const, candidates: [] };
		const result = validateNextSteps([{ type: "freetext", text: "Continue in prose" }], open, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0].reason).toContain("must include reason");
	});

	it("does not require reason for forced handoffs because forced validation stays outside selector validation", () => {
		expect(validateNextStep("mach12:pr-review-fix", closed)).toEqual({ valid: true });
	});

	it("does not select a recommendation when the index is missing", () => {
		const result = validateNextSteps([entry({ name: "mach12:pr-review-fix" })], closed);
		expect(result.recommended).toBeNull();
		expect(result.recommendedReason).toContain("missing recommended_next_step");
	});

	it("does not fall back to the first valid option when the recommended index is invalid", () => {
		const result = validateNextSteps([entry({ name: "mach12:pr-review-fix" })], closed, 2);
		expect(result.valid).toHaveLength(1);
		expect(result.recommended).toBeNull();
		expect(result.recommendedReason).toContain("outside next_steps");
	});

	it("rejects every entry under an ask policy", () => {
		const ask = { mode: "ask" as const, hint: "User picks" };
		const result = validateNextSteps([entry({ name: "x:y" }), entry({ name: "x:z" })], ask, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped.map((step) => step.label)).toEqual(["x:y", "x:z"]);
		expect(result.reason).toContain("ask");
	});
});
