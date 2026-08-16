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
	vi.doUnmock("os");
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

describe("getSelfUpdateCommand", () => {
	it.each([
		{
			name: "npm",
			execPath: (fixture: string) => join(fixture, "node_modules", "npm", "bin", "node"),
			output: (root: string) => root,
			args: ["install", "-g", "@leanandmean/scramjet@0.78.1"],
		},
		{
			name: "pnpm",
			execPath: (fixture: string) => join(fixture, "node_modules", ".pnpm", "node"),
			output: (root: string) => join(root, ".pnpm"),
			args: ["install", "-g", "@leanandmean/scramjet@0.78.1"],
		},
		{
			name: "yarn",
			execPath: (fixture: string) => join(fixture, ".yarn", "bin", "node"),
			output: (root: string) => join(root, ".."),
			args: ["global", "add", "@leanandmean/scramjet@0.78.1"],
		},
	])("pins the exact release for $name and captures the lexical manifest", async ({ execPath, output, args }) => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-update-command-"));
		const root = join(fixture, "global", "node_modules");
		const packageDir = join(root, "@leanandmean", "scramjet");
		mkdirSync(packageDir, { recursive: true });
		writeFileSync(join(packageDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		process.env.SCRAMJET_PACKAGE_DIR = packageDir;
		vi.spyOn(process, "execPath", "get").mockReturnValue(execPath(fixture));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 0, stdout: `${output(root)}\n`, stderr: "" })),
		}));

		const { getSelfUpdateCommand } = await import("../src/config.js");
		const command = getSelfUpdateCommand(
			"@leanandmean/coding-agent",
			"@leanandmean/scramjet",
			"@leanandmean/scramjet@0.78.1",
		);

		expect(command?.args).toEqual(args);
		expect(command?.targetManifestPath).toBe(join(root, "@leanandmean", "scramjet", "package.json"));
		expect(command?.steps?.[0].args.at(-1)).toBe("@leanandmean/coding-agent");
	});

	it("pins the exact release for Bun's lexical global root", async () => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-bun-command-"));
		const root = join(fixture, ".bun", "install", "global", "node_modules");
		const packageDir = join(root, "@leanandmean", "scramjet");
		mkdirSync(packageDir, { recursive: true });
		writeFileSync(join(packageDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		process.env.SCRAMJET_PACKAGE_DIR = packageDir;
		vi.spyOn(process, "execPath", "get").mockReturnValue(join(root, "bun"));
		vi.doMock("os", async (importOriginal) => ({
			...(await importOriginal<typeof import("os")>()),
			homedir: () => fixture,
		}));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 1, stdout: "", stderr: "no global project" })),
		}));

		const { getSelfUpdateCommand } = await import("../src/config.js");
		const command = getSelfUpdateCommand(
			"@leanandmean/scramjet",
			"@leanandmean/scramjet",
			"@leanandmean/scramjet@0.78.1",
		);

		expect(command?.command).toBe("bun");
		expect(command?.args).toEqual(["install", "-g", "@leanandmean/scramjet@0.78.1"]);
		expect(command?.targetManifestPath).toBe(join(packageDir, "package.json"));
	});

	it("retains the lexical manager root when ownership resolves through a symlink", async () => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-lexical-root-"));
		const realRoot = join(fixture, "real", "node_modules");
		const lexicalRoot = join(fixture, "global", "node_modules");
		const realPackageDir = join(realRoot, "@leanandmean", "scramjet");
		mkdirSync(realPackageDir, { recursive: true });
		mkdirSync(join(fixture, "global"));
		symlinkSync(realRoot, lexicalRoot, "dir");
		writeFileSync(join(realPackageDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		process.env.SCRAMJET_PACKAGE_DIR = join(lexicalRoot, "@leanandmean", "scramjet");
		vi.spyOn(process, "execPath", "get").mockReturnValue(join(fixture, "node_modules", "npm", "bin", "node"));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 0, stdout: `${lexicalRoot}\n`, stderr: "" })),
		}));

		const { getSelfUpdateCommand } = await import("../src/config.js");
		const command = getSelfUpdateCommand(
			"@leanandmean/scramjet",
			"@leanandmean/scramjet",
			"@leanandmean/scramjet@0.78.1",
		);

		expect(command?.targetManifestPath).toBe(join(lexicalRoot, "@leanandmean", "scramjet", "package.json"));
	});

	it("preserves configured npm arguments while pinning the install spec", async () => {
		const fixture = mkdtempSync(join(tmpdir(), "scramjet-configured-npm-"));
		const root = join(fixture, "node_modules");
		const packageDir = join(root, "@leanandmean", "scramjet");
		mkdirSync(packageDir, { recursive: true });
		writeFileSync(join(packageDir, "package.json"), JSON.stringify({ name: "@leanandmean/scramjet" }));
		process.env.SCRAMJET_PACKAGE_DIR = packageDir;
		vi.spyOn(process, "execPath", "get").mockReturnValue(join(fixture, "node_modules", "npm", "bin", "node"));
		vi.doMock("child_process", () => ({
			spawnSync: vi.fn(() => ({ status: 0, stdout: `${root}\n`, stderr: "" })),
		}));

		const { getSelfUpdateCommand } = await import("../src/config.js");
		const command = getSelfUpdateCommand(
			"@leanandmean/scramjet",
			"@leanandmean/scramjet",
			"@leanandmean/scramjet@0.78.1",
			["sudo", "npm", "--registry", "https://registry.example.test"],
		);

		expect(command?.command).toBe("sudo");
		expect(command?.args).toEqual([
			"npm",
			"--registry",
			"https://registry.example.test",
			"install",
			"-g",
			"@leanandmean/scramjet@0.78.1",
		]);
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
