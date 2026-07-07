import { describe, expect, it, vi } from "vitest";
import { parseSlashCommand, validateNextStep, validateNextSteps } from "../src/commands/validator.js";
import type { CommandStatusNextStep } from "../src/types.js";

describe("parseSlashCommand", () => {
	it("parses a bare slash command with no args", () => {
		expect(parseSlashCommand("/mach12:pr-merge")).toEqual({ name: "mach12:pr-merge", args: undefined });
	});

	it("parses a slash command with args", () => {
		expect(parseSlashCommand("/mach12:pr-review-fix 94 --review-comment 123")).toEqual({
			name: "mach12:pr-review-fix",
			args: "94 --review-comment 123",
		});
	});

	it("trims surrounding whitespace before parsing", () => {
		expect(parseSlashCommand("  /mach12:pr-merge 113  ")).toEqual({ name: "mach12:pr-merge", args: "113" });
	});

	it("returns null for a non-slash message", () => {
		expect(parseSlashCommand("Let's discuss the failures")).toBeNull();
	});

	it("returns null for a bare slash", () => {
		expect(parseSlashCommand("/")).toBeNull();
	});

	it("returns null for a slash followed by a space (empty command name)", () => {
		expect(parseSlashCommand("/ foo")).toBeNull();
	});

	it("collapses args that are only whitespace to undefined", () => {
		expect(parseSlashCommand("/mach12:pr-merge   ")).toEqual({ name: "mach12:pr-merge", args: undefined });
	});
});

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

describe("validateNextSteps — commandCheck parameter", () => {
	const closed = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	const entry = (over: Partial<CommandStatusNextStep>): CommandStatusNextStep => ({
		message: "/mach12:pr-review-fix",
		fresh_session: false,
		reason: "fix review findings",
		...over,
	});

	it("passes all entries when no commandCheck is provided", () => {
		const result = validateNextSteps([entry({})], closed, 0);
		expect(result.valid).toHaveLength(1);
		expect(result.skipped).toEqual([]);
	});

	it("passes entries when commandCheck returns null", () => {
		const check = () => null;
		const result = validateNextSteps([entry({})], closed, 0, check);
		expect(result.valid).toHaveLength(1);
		expect(result.skipped).toEqual([]);
	});

	it("skips entries when commandCheck returns a rejection reason", () => {
		const check = (name: string) => (name === "mach12:pr-review-fix" ? "delegate-only command" : null);
		const result = validateNextSteps(
			[entry({}), entry({ message: "/mach12:pr-pre-merge", reason: "merge" })],
			closed,
			0,
			check,
		);
		expect(result.valid).toHaveLength(1);
		expect(result.valid[0].parsedCommand?.name).toBe("mach12:pr-pre-merge");
		expect(result.skipped).toHaveLength(1);
		expect(result.skipped[0].reason).toBe("delegate-only command");
	});

	it("demotes recommended index when it points to a command-checked entry", () => {
		const check = (name: string) => (name === "mach12:pr-review-fix" ? "delegate-only" : null);
		const result = validateNextSteps(
			[entry({}), entry({ message: "/mach12:pr-pre-merge", reason: "merge" })],
			closed,
			0,
			check,
		);
		expect(result.recommended).toBeNull();
		expect(result.recommendedReason).toContain("points to invalid next step");
	});

	it("does not apply commandCheck to non-command messages", () => {
		const open = { mode: "open" as const, candidates: [] };
		const check = vi.fn(() => "should not be called");
		const result = validateNextSteps(
			[{ message: "Plain text suggestion", fresh_session: false, reason: "context" }],
			open,
			0,
			check,
		);
		expect(check).not.toHaveBeenCalled();
		expect(result.valid).toHaveLength(1);
	});
});

describe("validateNextSteps — same-command-different-args messages", () => {
	const closed = {
		mode: "closed" as const,
		candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
	};

	const genuineOnly: CommandStatusNextStep = {
		message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456",
		fresh_session: true,
		reason: "Address genuine issues only",
	};

	const withNitpicks: CommandStatusNextStep = {
		message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456 --include-nitpicks",
		fresh_session: true,
		reason: "Address genuine issues and nitpicks",
	};

	const preMerge: CommandStatusNextStep = {
		message: "/mach12:pr-pre-merge 94",
		fresh_session: true,
		reason: "No issues found, proceed to merge",
	};

	it("accepts both same-command messages with different args under closed validation", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks], closed, 0);
		expect(result.valid).toHaveLength(2);
		expect(result.skipped).toEqual([]);
	});

	it("assigns distinct indexes to same-command entries", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 0);
		expect(result.valid.map((o) => o.index)).toEqual([0, 1, 2]);
	});

	it("preserves each entry's full message and parsed args", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks], closed, 0);
		expect(result.valid[0].message).toBe(genuineOnly.message);
		expect(result.valid[1].message).toBe(withNitpicks.message);
		expect(result.valid[0].parsedCommand?.args).toBe("94 --review-comment 123 --assessment-comment 456");
		expect(result.valid[1].parsedCommand?.args).toBe(
			"94 --review-comment 123 --assessment-comment 456 --include-nitpicks",
		);
	});

	it("recommendation correctly targets the second same-command entry", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 1);
		expect(result.recommended).toMatchObject({
			index: 1,
			reason: "Address genuine issues and nitpicks",
			parsedCommand: { name: "mach12:pr-review-fix" },
		});
	});

	it("recommendation correctly targets the first same-command entry", () => {
		const result = validateNextSteps([genuineOnly, withNitpicks, preMerge], closed, 0);
		expect(result.recommended).toMatchObject({
			index: 0,
			reason: "Address genuine issues only",
			parsedCommand: { name: "mach12:pr-review-fix" },
		});
	});

	it("same-command messages with different args pass open validation", () => {
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

	const entry = (over: Partial<CommandStatusNextStep>): CommandStatusNextStep => ({
		message: "/mach12:pr-review-fix",
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
				entry({ message: "/mach12:pr-review-fix 55", fresh_session: true, reason: "fix review findings" }),
				entry({ message: "/mach12:pr-pre-merge", reason: "review is complete" }),
			],
			closed,
			1,
		);

		expect(result.valid).toHaveLength(2);
		expect(result.recommended).toEqual({
			index: 1,
			reason: "review is complete",
			message: "/mach12:pr-pre-merge",
			freshSession: false,
			parsedCommand: { name: "mach12:pr-pre-merge", args: undefined },
		});
		expect(result.skipped).toEqual([]);
	});

	it("maps fresh_session false to freshSession false", () => {
		const result = validateNextSteps([entry({ fresh_session: false })], closed, 0);
		expect(result.recommended?.freshSession).toBe(false);
	});

	it("records skipped invalid entries with reasons without changing recommendation indexes", () => {
		const result = validateNextSteps(
			[entry({ message: "/z:not-in-list" }), entry({ message: "/mach12:pr-pre-merge" })],
			closed,
			1,
		);

		expect(result.valid.map((option) => option.index)).toEqual([1]);
		expect(result.recommended).toMatchObject({ index: 1, parsedCommand: { name: "mach12:pr-pre-merge" } });
		expect(result.skipped).toEqual([
			{
				index: 0,
				label: "/z:not-in-list",
				reason: expect.stringContaining("not in closed candidates"),
			},
		]);
	});

	it("rejects every invalid entry and keeps the first rejection reason", () => {
		const result = validateNextSteps([entry({ message: "/z:one" }), entry({ message: "/z:two" })], closed, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped.map((step) => step.label)).toEqual(["/z:one", "/z:two"]);
		expect(result.reason).toContain("z:one");
		expect(result.recommendedReason).toContain("recommended_next_step 0 points to invalid next step /z:one");
	});

	it("accepts open command and non-command message candidates", () => {
		const open = { mode: "open" as const, candidates: [{ name: "mach12:issue-review" }] };
		const result = validateNextSteps(
			[
				entry({ message: "/infra:rotate-key", reason: "outside Scramjet but useful" }),
				{
					message: "Please summarize the issue.",
					fresh_session: false,
					reason: "needs more context",
				},
			],
			open,
			1,
		);

		expect(result.valid).toHaveLength(2);
		expect(result.recommended).toEqual({
			index: 1,
			reason: "needs more context",
			message: "Please summarize the issue.",
			freshSession: false,
			parsedCommand: null,
		});
	});

	it("rejects blacklisted open command candidates", () => {
		const open = {
			mode: "open" as const,
			candidates: [{ name: "mach12:issue-review" }],
			blacklist: ["mach12:pr-merge"],
		};
		const result = validateNextSteps([entry({ message: "/mach12:pr-merge" })], open, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0]).toMatchObject({
			label: "/mach12:pr-merge",
			reason: expect.stringContaining("blacklisted"),
		});
	});

	it("rejects non-command messages outside open policies", () => {
		const result = validateNextSteps(
			[{ message: "Continue in prose", fresh_session: false, reason: "not a command" }],
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

	it("rejects missing reason for selector-visible non-command entries", () => {
		const open = { mode: "open" as const, candidates: [] };
		const result = validateNextSteps([{ message: "Continue in prose", fresh_session: false }], open, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped[0].reason).toContain("must include reason");
	});

	it("does not require reason for forced handoffs because forced validation stays outside selector validation", () => {
		expect(validateNextStep("mach12:pr-review-fix", closed)).toEqual({ valid: true });
	});

	it("does not select a recommendation when the index is missing", () => {
		const result = validateNextSteps([entry({})], closed);
		expect(result.recommended).toBeNull();
		expect(result.recommendedReason).toContain("missing recommended_next_step");
	});

	it("does not fall back to the first valid option when the recommended index is invalid", () => {
		const result = validateNextSteps([entry({})], closed, 2);
		expect(result.valid).toHaveLength(1);
		expect(result.recommended).toBeNull();
		expect(result.recommendedReason).toContain("outside next_steps");
	});

	it("rejects every entry under an ask policy", () => {
		const ask = { mode: "ask" as const, hint: "User picks" };
		const result = validateNextSteps([entry({ message: "/x:y" }), entry({ message: "/x:z" })], ask, 0);
		expect(result.valid).toEqual([]);
		expect(result.skipped.map((step) => step.label)).toEqual(["/x:y", "/x:z"]);
		expect(result.reason).toContain("ask");
	});
});
