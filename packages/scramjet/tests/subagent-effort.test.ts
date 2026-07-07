import { describe, expect, it } from "vitest";
import { capThinkingLevel } from "../src/subagent/index.js";

describe("capThinkingLevel", () => {
	it("returns requested when below parent", () => {
		expect(capThinkingLevel("low", "high")).toBe("low");
	});

	it("returns requested when equal to parent", () => {
		expect(capThinkingLevel("medium", "medium")).toBe("medium");
	});

	it("caps to parent when requested exceeds parent", () => {
		expect(capThinkingLevel("xhigh", "medium")).toBe("medium");
	});

	it("handles 'off' as parent (lowest possible)", () => {
		expect(capThinkingLevel("high", "off")).toBe("off");
	});

	it("handles 'off' as requested (always passes through)", () => {
		expect(capThinkingLevel("off", "high")).toBe("off");
	});

	it("handles 'xhigh' parent (never caps)", () => {
		expect(capThinkingLevel("xhigh", "xhigh")).toBe("xhigh");
		expect(capThinkingLevel("high", "xhigh")).toBe("high");
	});

	it("caps minimal to off when parent is off", () => {
		expect(capThinkingLevel("minimal", "off")).toBe("off");
	});

	it("passes through each level when parent is xhigh", () => {
		const levels = ["off", "minimal", "low", "medium", "high", "xhigh"] as const;
		for (const level of levels) {
			expect(capThinkingLevel(level, "xhigh")).toBe(level);
		}
	});
});
