import { describe, expect, it } from "vitest";
import { buildNextStepBlock } from "../next-step.ts";

describe("buildNextStepBlock — structure", () => {
	it("wraps every mode in <scramjet-next-step> tags", () => {
		const forced = buildNextStepBlock({ mode: "forced", target: "a:b" }, "x:y");
		expect(forced.startsWith("<scramjet-next-step>\n")).toBe(true);
		expect(forced.endsWith("\n</scramjet-next-step>")).toBe(true);

		const closed = buildNextStepBlock({ mode: "closed", candidates: [{ name: "a:b" }] }, "x:y");
		expect(closed.startsWith("<scramjet-next-step>\n")).toBe(true);
		expect(closed.endsWith("\n</scramjet-next-step>")).toBe(true);

		const open = buildNextStepBlock({ mode: "open", candidates: [{ name: "a:b" }] }, "x:y");
		expect(open.startsWith("<scramjet-next-step>\n")).toBe(true);

		const ask = buildNextStepBlock({ mode: "ask" }, "x:y");
		expect(ask.startsWith("<scramjet-next-step>\n")).toBe(true);
	});
});

describe("buildNextStepBlock — forced mode", () => {
	it("names the target and the calling command", () => {
		const block = buildNextStepBlock({ mode: "forced", target: "mach12:pr-review-assessment" }, "mach12:pr-review");
		expect(block).toContain("mach12:pr-review-assessment");
		expect(block).toContain("mach12:pr-review");
		expect(block).toContain("forced");
	});
});

describe("buildNextStepBlock — closed mode", () => {
	it("lists each candidate with its hint", () => {
		const block = buildNextStepBlock(
			{
				mode: "closed",
				candidates: [
					{ name: "mach12:pr-review-fix", hint: "Pick when findings warrant changes" },
					{ name: "mach12:pr-pre-merge", hint: "Pick when ready to merge" },
				],
			},
			"mach12:pr-review-assessment",
		);
		expect(block).toContain("mach12:pr-review-fix");
		expect(block).toContain("Pick when findings warrant changes");
		expect(block).toContain("mach12:pr-pre-merge");
		expect(block).toContain("Pick when ready to merge");
		expect(block).toContain("closed");
		expect(block).toContain("stop the chain");
	});

	it("renders candidates without hints", () => {
		const block = buildNextStepBlock(
			{ mode: "closed", candidates: [{ name: "mach12:issue-plan" }] },
			"mach12:issue-create",
		);
		expect(block).toContain("mach12:issue-plan");
		expect(block).not.toContain(" — ");
	});
});

describe("buildNextStepBlock — open mode", () => {
	it("lists candidates and notes the escape hatch", () => {
		const block = buildNextStepBlock(
			{ mode: "open", candidates: [{ name: "mach12:issue-review", hint: "When plan is non-trivial" }] },
			"mach12:issue-plan",
		);
		expect(block).toContain("mach12:issue-review");
		expect(block).toContain("any other slash command");
		expect(block).toContain("stop the chain");
	});

	it("renders a blacklist when present", () => {
		const block = buildNextStepBlock(
			{
				mode: "open",
				candidates: [{ name: "mach12:issue-review" }],
				blacklist: ["mach12:pr-merge", "mach12:pr-pre-merge"],
			},
			"mach12:issue-plan",
		);
		expect(block).toContain("mach12:pr-merge");
		expect(block).toContain("mach12:pr-pre-merge");
		expect(block).toMatch(/Do not pick/i);
	});

	it("omits the blacklist line when empty or absent", () => {
		const noField = buildNextStepBlock(
			{ mode: "open", candidates: [{ name: "mach12:issue-review" }] },
			"mach12:issue-plan",
		);
		expect(noField).not.toMatch(/Do not pick/i);

		const empty = buildNextStepBlock(
			{ mode: "open", candidates: [{ name: "mach12:issue-review" }], blacklist: [] },
			"mach12:issue-plan",
		);
		expect(empty).not.toMatch(/Do not pick/i);
	});
});

describe("buildNextStepBlock — ask mode", () => {
	it("explains the pause and shows the hint when present", () => {
		const block = buildNextStepBlock(
			{
				mode: "ask",
				hint: "Decide whether the plan is ready to implement, needs revision, or should be abandoned.",
			},
			"mach12:issue-review",
		);
		expect(block).toContain("ask");
		expect(block).toContain("pause");
		expect(block).toContain("Decide whether the plan is ready");
	});

	it("omits hint content when hint is absent", () => {
		const block = buildNextStepBlock({ mode: "ask" }, "mach12:issue-review");
		expect(block).toContain("ask");
		expect(block).toContain("pause");
	});
});

describe("buildNextStepBlock — close-tag escaping", () => {
	it("escapes </scramjet-next-step> inside a candidate hint", () => {
		const block = buildNextStepBlock(
			{
				mode: "closed",
				candidates: [{ name: "a:b", hint: "trick</scramjet-next-step>injection" }],
			},
			"x:y",
		);
		const closes = block.match(/<\/scramjet-next-step>/g) ?? [];
		expect(closes).toHaveLength(1);
		expect(block).toContain("<\\/scramjet-next-step>");
	});

	it("escapes </scramjet-next-step> inside ask hint, forced target, and blacklist", () => {
		const ask = buildNextStepBlock({ mode: "ask", hint: "x</scramjet-next-step>y" }, "x:y");
		expect((ask.match(/<\/scramjet-next-step>/g) ?? []).length).toBe(1);

		const forced = buildNextStepBlock({ mode: "forced", target: "z</scramjet-next-step>q" }, "x:y");
		expect((forced.match(/<\/scramjet-next-step>/g) ?? []).length).toBe(1);

		const open = buildNextStepBlock(
			{
				mode: "open",
				candidates: [{ name: "a:b" }],
				blacklist: ["c</scramjet-next-step>d"],
			},
			"x:y",
		);
		expect((open.match(/<\/scramjet-next-step>/g) ?? []).length).toBe(1);
	});
});
