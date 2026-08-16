import { describe, expect, it, vi } from "vitest";
import { isNewerPackageVersion, resolveCurrentRelease } from "../src/utils/version-check.js";

const success = (stdout: string) => ({ stdout, stderr: "", code: 0, killed: false });

describe("resolveCurrentRelease", () => {
	it("resolves one validated release with exact bounded npm arguments", async () => {
		const executor = vi.fn(async () => success('"0.78.1"'));

		await expect(resolveCurrentRelease("@leanandmean/scramjet", executor, 5000)).resolves.toEqual({
			packageName: "@leanandmean/scramjet",
			version: "0.78.1",
		});
		expect(executor).toHaveBeenCalledWith("npm", ["view", "@leanandmean/scramjet", "version", "--json"], {
			timeout: 5000,
		});
	});

	it.each([
		["nonzero exit", { ...success('"0.78.1"'), stderr: "registry unavailable", code: 1 }, "registry unavailable"],
		["killed process", { ...success('"0.78.1"'), code: 1, killed: true }, "timed out after 5000ms"],
		["malformed JSON", success("{"), "malformed JSON"],
		["non-string JSON", success('["0.78.1"]'), "invalid package version"],
		["invalid version", success('"latest"'), "invalid package version"],
	])("rejects %s", async (_label, result, message) => {
		await expect(resolveCurrentRelease("@leanandmean/scramjet", async () => result, 5000)).rejects.toThrow(message);
	});
});

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
		["9007199254740993.0.0", "9007199254740992.0.0", true],
		["1.9007199254740993.0", "1.9007199254740992.0", true],
		["1.0.9007199254740993", "1.0.9007199254740992", true],
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
