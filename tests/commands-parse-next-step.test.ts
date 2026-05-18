import { describe, expect, it } from "vitest";
import { parseNextStepPolicy } from "../commands/parse-next-step.ts";

describe("parseNextStepPolicy — absent next block", () => {
	it("returns null policy when frontmatter has no next field", () => {
		expect(parseNextStepPolicy({})).toEqual({ ok: true, policy: null });
	});

	it("returns null policy when next is explicitly null", () => {
		expect(parseNextStepPolicy({ next: null })).toEqual({ ok: true, policy: null });
	});
});

describe("parseNextStepPolicy — forced mode", () => {
	it("parses a valid forced policy", () => {
		const result = parseNextStepPolicy({ next: { mode: "forced", target: "mach12:pr-review-assessment" } });
		expect(result).toEqual({ ok: true, policy: { mode: "forced", target: "mach12:pr-review-assessment" } });
	});

	it("rejects forced when target is missing", () => {
		const result = parseNextStepPolicy({ next: { mode: "forced" } });
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toContain("target");
	});

	it("rejects forced when target is empty", () => {
		const result = parseNextStepPolicy({ next: { mode: "forced", target: "  " } });
		expect(result.ok).toBe(false);
	});
});

describe("parseNextStepPolicy — closed mode", () => {
	it("parses candidates with hints", () => {
		const result = parseNextStepPolicy({
			next: {
				mode: "closed",
				candidates: [
					{ name: "mach12:pr-review-fix", hint: "Pick when findings warrant changes" },
					{ name: "mach12:pr-pre-merge", hint: "Pick when ready to merge" },
				],
			},
		});
		expect(result).toEqual({
			ok: true,
			policy: {
				mode: "closed",
				candidates: [
					{ name: "mach12:pr-review-fix", hint: "Pick when findings warrant changes" },
					{ name: "mach12:pr-pre-merge", hint: "Pick when ready to merge" },
				],
			},
		});
	});

	it("parses candidates without hints", () => {
		const result = parseNextStepPolicy({
			next: { mode: "closed", candidates: [{ name: "mach12:issue-plan" }] },
		});
		expect(result).toEqual({
			ok: true,
			policy: { mode: "closed", candidates: [{ name: "mach12:issue-plan" }] },
		});
	});

	it("rejects closed without candidates list", () => {
		const result = parseNextStepPolicy({ next: { mode: "closed" } });
		expect(result.ok).toBe(false);
	});

	it("rejects candidates that are not a list", () => {
		const result = parseNextStepPolicy({ next: { mode: "closed", candidates: "mach12:foo" } });
		expect(result.ok).toBe(false);
	});

	it("rejects candidate entries with missing name", () => {
		const result = parseNextStepPolicy({
			next: { mode: "closed", candidates: [{ hint: "no name here" }] },
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toContain("name");
	});

	it("rejects an empty candidates list (agent would have nothing to pick from)", () => {
		const result = parseNextStepPolicy({ next: { mode: "closed", candidates: [] } });
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toContain("empty");
	});
});

describe("parseNextStepPolicy — open mode", () => {
	it("parses with no blacklist", () => {
		const result = parseNextStepPolicy({
			next: { mode: "open", candidates: [{ name: "mach12:issue-plan" }] },
		});
		expect(result).toEqual({
			ok: true,
			policy: { mode: "open", candidates: [{ name: "mach12:issue-plan" }] },
		});
	});

	it("parses with a blacklist", () => {
		const result = parseNextStepPolicy({
			next: {
				mode: "open",
				candidates: [{ name: "mach12:issue-review" }],
				blacklist: ["mach12:pr-merge"],
			},
		});
		expect(result).toEqual({
			ok: true,
			policy: {
				mode: "open",
				candidates: [{ name: "mach12:issue-review" }],
				blacklist: ["mach12:pr-merge"],
			},
		});
	});

	it("rejects a non-list blacklist", () => {
		const result = parseNextStepPolicy({
			next: { mode: "open", candidates: [{ name: "mach12:x" }], blacklist: "mach12:y" },
		});
		expect(result.ok).toBe(false);
	});
});

describe("parseNextStepPolicy — ask mode", () => {
	it("parses ask with no hint", () => {
		const result = parseNextStepPolicy({ next: { mode: "ask" } });
		expect(result).toEqual({ ok: true, policy: { mode: "ask" } });
	});

	it("parses ask with a hint", () => {
		const result = parseNextStepPolicy({
			next: { mode: "ask", hint: "Decide whether to approve, revise, or abandon." },
		});
		expect(result).toEqual({
			ok: true,
			policy: { mode: "ask", hint: "Decide whether to approve, revise, or abandon." },
		});
	});

	it("rejects ask with a non-string hint", () => {
		const result = parseNextStepPolicy({ next: { mode: "ask", hint: 42 } });
		expect(result.ok).toBe(false);
	});
});

describe("parseNextStepPolicy — malformed input", () => {
	it("rejects unknown mode", () => {
		const result = parseNextStepPolicy({ next: { mode: "auto-detect" } });
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toContain("auto-detect");
	});

	it("rejects mode that is not a string", () => {
		const result = parseNextStepPolicy({ next: { mode: 7 } });
		expect(result.ok).toBe(false);
	});

	it("rejects next that is a list, not a mapping", () => {
		const result = parseNextStepPolicy({ next: ["forced", "mach12:x"] });
		expect(result.ok).toBe(false);
	});

	it("rejects next that is a scalar", () => {
		const result = parseNextStepPolicy({ next: "forced" });
		expect(result.ok).toBe(false);
	});
});
