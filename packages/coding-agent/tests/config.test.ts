import { afterEach, describe, expect, it, vi } from "vitest";

const SCRAMJET_PACKAGE_NAME = "SCRAMJET_PACKAGE_NAME";
const SCRAMJET_CHANGELOG_PATH = "SCRAMJET_CHANGELOG_PATH";
const originalPackageName = process.env[SCRAMJET_PACKAGE_NAME];
const originalChangelogPath = process.env[SCRAMJET_CHANGELOG_PATH];

afterEach(() => {
	if (originalPackageName === undefined) {
		delete process.env[SCRAMJET_PACKAGE_NAME];
	} else {
		process.env[SCRAMJET_PACKAGE_NAME] = originalPackageName;
	}
	if (originalChangelogPath === undefined) {
		delete process.env[SCRAMJET_CHANGELOG_PATH];
	} else {
		process.env[SCRAMJET_CHANGELOG_PATH] = originalChangelogPath;
	}
	vi.resetModules();
});

describe("PACKAGE_NAME", () => {
	it("defaults to the coding-agent package name", async () => {
		delete process.env[SCRAMJET_PACKAGE_NAME];
		vi.resetModules();

		const { PACKAGE_NAME } = await import("../src/config.js");

		expect(PACKAGE_NAME).toBe("@leanandmean/coding-agent");
	});

	it("allows the Scramjet bin to override the self-update package", async () => {
		process.env[SCRAMJET_PACKAGE_NAME] = "@leanandmean/scramjet";
		vi.resetModules();

		const { PACKAGE_NAME } = await import("../src/config.js");

		expect(PACKAGE_NAME).toBe("@leanandmean/scramjet");
	});
});

describe("getChangelogPath", () => {
	it("returns the package CHANGELOG.md by default", async () => {
		delete process.env[SCRAMJET_CHANGELOG_PATH];
		vi.resetModules();

		const { getChangelogPath } = await import("../src/config.js");

		expect(getChangelogPath()).toMatch(/CHANGELOG\.md$/);
	});

	it("uses SCRAMJET_CHANGELOG_PATH when set", async () => {
		process.env[SCRAMJET_CHANGELOG_PATH] = "/custom/path/CHANGELOG.md";
		vi.resetModules();

		const { getChangelogPath } = await import("../src/config.js");

		expect(getChangelogPath()).toBe("/custom/path/CHANGELOG.md");
	});
});
