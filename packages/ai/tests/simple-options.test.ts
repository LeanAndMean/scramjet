import { describe, expect, it } from "vitest";
import { clampReasoning } from "../src/providers/simple-options.js";

describe("clampReasoning", () => {
	it("clamps xhigh to high", () => {
		expect(clampReasoning("xhigh")).toBe("high");
	});

	it("clamps max to high", () => {
		expect(clampReasoning("max")).toBe("high");
	});

	it("passes through other levels unchanged", () => {
		expect(clampReasoning("low")).toBe("low");
		expect(clampReasoning("medium")).toBe("medium");
		expect(clampReasoning("high")).toBe("high");
	});

	it("passes through undefined", () => {
		expect(clampReasoning(undefined)).toBeUndefined();
	});
});
