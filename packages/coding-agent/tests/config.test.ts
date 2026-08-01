import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { afterEach, describe, expect, it, vi } from "vitest";

const SCRAMJET_PACKAGE_NAME = "SCRAMJET_PACKAGE_NAME";
const SCRAMJET_CHANGELOG_PATH = "SCRAMJET_CHANGELOG_PATH";
const SCRAMJET_PACKAGE_DIR = "SCRAMJET_PACKAGE_DIR";
const originalPackageName = process.env[SCRAMJET_PACKAGE_NAME];
const originalChangelogPath = process.env[SCRAMJET_CHANGELOG_PATH];
const originalPackageDir = process.env[SCRAMJET_PACKAGE_DIR];

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
	if (originalPackageDir === undefined) {
		delete process.env[SCRAMJET_PACKAGE_DIR];
	} else {
		process.env[SCRAMJET_PACKAGE_DIR] = originalPackageDir;
	}
	vi.restoreAllMocks();
	vi.doUnmock("child_process");
	vi.doUnmock("fs");
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

describe("isCurrentInstallationManaged", () => {
	it("recognizes a package inside a global package root without checking writability", async () => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-managed-install-"));
		const root = join(fixture, "lib", "node_modules");
		const packageDir = join(root, "@leanandmean", "scramjet");
		mkdirSync(packageDir, { recursive: true });
		writeFileSync(join(packageDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		process.env.SCRAMJET_PACKAGE_DIR = packageDir;
		vi.spyOn(process, "execPath", "get").mockReturnValue(join(fixture, "node_modules", "npm", "bin", "node"));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 0, stdout: `${root}\n`, stderr: "" })),
		}));
		vi.doMock("fs", async (importOriginal) => ({
			...(await importOriginal<typeof import("fs")>()),
			accessSync: vi.fn(() => {
				throw new Error("unwritable fixture");
			}),
		}));

		const { isCurrentInstallationManaged } = await import("../src/config.js");

		expect(isCurrentInstallationManaged()).toBe(true);
	});

	it("rejects a package symlink that resolves outside the global package root", async () => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-source-install-"));
		const root = join(fixture, "lib", "node_modules");
		const sourceDir = join(fixture, "checkout");
		const packageDir = join(root, "@leanandmean", "scramjet");
		mkdirSync(join(root, "@leanandmean"), { recursive: true });
		mkdirSync(sourceDir);
		writeFileSync(join(sourceDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		symlinkSync(sourceDir, packageDir, "dir");
		process.env.SCRAMJET_PACKAGE_DIR = packageDir;
		vi.spyOn(process, "execPath", "get").mockReturnValue(join(fixture, "node_modules", "npm", "bin", "node"));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 0, stdout: `${root}\n`, stderr: "" })),
		}));

		const { isCurrentInstallationManaged } = await import("../src/config.js");

		expect(isCurrentInstallationManaged()).toBe(false);
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
