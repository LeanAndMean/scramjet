import { afterEach, describe, expect, it, vi } from "vitest";

const SCRAMJET_PACKAGE_NAME = "SCRAMJET_PACKAGE_NAME";
const originalPackageName = process.env[SCRAMJET_PACKAGE_NAME];

afterEach(() => {
	if (originalPackageName === undefined) {
		delete process.env[SCRAMJET_PACKAGE_NAME];
	} else {
		process.env[SCRAMJET_PACKAGE_NAME] = originalPackageName;
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
