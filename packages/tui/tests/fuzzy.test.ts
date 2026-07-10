import { describe, expect, it } from "vitest";
import { fuzzyFilter, fuzzyMatch } from "../src/fuzzy.js";

describe("fuzzyFilter", () => {
	it("empty query returns items unchanged", () => {
		const items = ["alpha", "beta", "gamma"];
		expect(fuzzyFilter(items, "", (s) => s)).toEqual(["alpha", "beta", "gamma"]);
		expect(fuzzyFilter(items, "   ", (s) => s)).toEqual(["alpha", "beta", "gamma"]);
	});

	it("non-matching query returns empty", () => {
		const items = ["alpha", "beta", "gamma"];
		expect(fuzzyFilter(items, "xyz", (s) => s)).toEqual([]);
	});

	it("tied scores prefer shorter text", () => {
		const items = ["mach12:pr-review-assessment", "mach12:pr-review", "mach12:pr-review-fix"];
		const result = fuzzyFilter(items, "pr-rev", (s) => s);
		expect(result[0]).toBe("mach12:pr-review");
	});

	it("better score wins over shorter length", () => {
		const short = "xyzpr-rev";
		const long = "mach12:pr-review-assessment";
		const items = [long, short];
		const result = fuzzyFilter(items, "pr-rev", (s) => s);
		const shortScore = fuzzyMatch("pr-rev", short).score;
		const longScore = fuzzyMatch("pr-rev", long).score;
		expect(longScore).toBeLessThan(shortScore);
		expect(result[0]).toBe(long);
	});
});
