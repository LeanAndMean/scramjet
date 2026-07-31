import { describe, expect, it } from "vitest";
import { isNewerPackageVersion } from "../src/utils/version-check.js";

describe("isNewerPackageVersion", () => {
	it.each([
		["2.0.0", "1.9.9", true],
		["1.2.0", "1.1.9", true],
		["1.2.3", "1.2.2", true],
		["1.2.3", "1.2.3-beta.1", true],
		["1.2.3-beta.2", "1.2.3-beta.1", true],
		["1.2.3-beta.10", "1.2.3-beta.2", true],
		["1.2.3-alpha.beta", "1.2.3-alpha.1", true],
		["1.2.3-alpha.1", "1.2.3-alpha", true],
		["1.2.3", "1.2.3", false],
		["1.2.2", "1.2.3", false],
		["1.2.3+build.2", "1.2.3+build.1", false],
	])("compares %s against %s", (candidate, current, expected) => {
		expect(isNewerPackageVersion(candidate, current)).toBe(expected);
	});

	it.each([
		["", "1.2.3"],
		["invalid", "1.2.3"],
		["1.2.3", ""],
		["1.2.3", "invalid"],
		["invalid-a", "invalid-b"],
		["01.2.3", "1.2.2"],
		["1.2.3-beta.01", "1.2.3-beta.1"],
		["1.2.3+", "1.2.2"],
	])("fails closed for malformed versions %j and %j", (candidate, current) => {
		expect(isNewerPackageVersion(candidate, current)).toBe(false);
	});
});
