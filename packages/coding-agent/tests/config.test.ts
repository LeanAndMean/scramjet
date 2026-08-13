import { mkdirSync, mkdtempSync, readlinkSync, symlinkSync, unlinkSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { dirname, join, relative } from "path";
import { afterEach, describe, expect, it, vi } from "vitest";

const SCRAMJET_PACKAGE_NAME = "SCRAMJET_PACKAGE_NAME";
const SCRAMJET_CHANGELOG_PATH = "SCRAMJET_CHANGELOG_PATH";
const SCRAMJET_PACKAGE_DIR = "SCRAMJET_PACKAGE_DIR";
const SCRAMJET_INTERNAL_PRODUCT_ROOT = "SCRAMJET_INTERNAL_PRODUCT_ROOT";
const originalPackageName = process.env[SCRAMJET_PACKAGE_NAME];
const originalChangelogPath = process.env[SCRAMJET_CHANGELOG_PATH];
const originalPackageDir = process.env[SCRAMJET_PACKAGE_DIR];
const originalProductRoot = process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT];

interface NpmFixture {
	fixture: string;
	root: string;
	productRoot: string;
	codingAgentRoot: string;
	manifestPath: string;
	binTargetPath: string;
	launcherPath: string;
	launcherLinkText: string;
}

function createNpmFixture(layout: "product-only" | "prepopulated" | "hoisted-only" = "product-only"): NpmFixture {
	const fixture = mkdtempSync(join(tmpdir(), "scramjet-npm-recovery-"));
	const root = join(fixture, "lib", "node_modules");
	const productRoot = join(root, "@leanandmean", "scramjet");
	const nestedCodingAgentRoot = join(productRoot, "node_modules", "@leanandmean", "coding-agent");
	const topLevelCodingAgentRoot = join(root, "@leanandmean", "coding-agent");
	const codingAgentRoot = layout === "hoisted-only" ? topLevelCodingAgentRoot : nestedCodingAgentRoot;
	const manifestPath = join(productRoot, "package.json");
	const binTargetPath = join(productRoot, "bin", "scramjet.js");
	const launcherPath = join(fixture, "bin", "scramjet");
	const launcherLinkText = relative(dirname(launcherPath), binTargetPath);
	mkdirSync(codingAgentRoot, { recursive: true });
	mkdirSync(dirname(binTargetPath), { recursive: true });
	mkdirSync(dirname(launcherPath), { recursive: true });
	writeFileSync(
		manifestPath,
		JSON.stringify({ name: "@leanandmean/scramjet", bin: { scramjet: "./bin/scramjet.js" } }),
	);
	writeFileSync(join(codingAgentRoot, "package.json"), JSON.stringify({ name: "@leanandmean/coding-agent" }));
	if (layout === "prepopulated") {
		mkdirSync(topLevelCodingAgentRoot, { recursive: true });
		writeFileSync(
			join(topLevelCodingAgentRoot, "package.json"),
			JSON.stringify({ name: "@leanandmean/coding-agent" }),
		);
	}
	writeFileSync(binTargetPath, "#!/usr/bin/env node\n");
	symlinkSync(launcherLinkText, launcherPath);
	process.env[SCRAMJET_PACKAGE_DIR] = productRoot;
	process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT] = productRoot;
	vi.spyOn(process, "execPath", "get").mockReturnValue(join(fixture, "node_modules", "npm", "bin", "node"));
	vi.doMock("url", async (importOriginal) => ({
		...(await importOriginal<typeof import("url")>()),
		fileURLToPath: vi.fn(() => join(codingAgentRoot, "dist", "config.js")),
	}));
	vi.doMock("child_process", () => ({
		spawnSync: vi.fn(() => ({ status: 0, stdout: `${root}\n`, stderr: "" })),
	}));
	return { fixture, root, productRoot, codingAgentRoot, manifestPath, binTargetPath, launcherPath, launcherLinkText };
}

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
	if (originalProductRoot === undefined) {
		delete process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT];
	} else {
		process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT] = originalProductRoot;
	}
	vi.restoreAllMocks();
	vi.doUnmock("child_process");
	vi.doUnmock("fs");
	vi.doUnmock("url");
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

describe("getSelfUpdateCommand npm recovery qualification", () => {
	it("attaches immutable recovery identity for the qualified product-only npm layout", async () => {
		const fixture = createNpmFixture();
		const { getSelfUpdateCommand } = await import("../src/config.js");

		const command = getSelfUpdateCommand("@leanandmean/scramjet");

		expect(command?.npmRecovery).toEqual({
			packageName: "@leanandmean/scramjet",
			productRoot: fixture.productRoot,
			packageRootType: "directory",
			runtimeRoot: fixture.codingAgentRoot,
			manifestPath: fixture.manifestPath,
			declaredBinPath: "./bin/scramjet.js",
			binTargetPath: fixture.binTargetPath,
			launcherPath: fixture.launcherPath,
			launcherType: "symbolic-link",
			launcherLinkText: fixture.launcherLinkText,
			launcherTargetPath: fixture.binTargetPath,
			productParentPath: dirname(fixture.productRoot),
			launcherParentPath: dirname(fixture.launcherPath),
			productDevice: expect.any(Number),
			productParentDevice: expect.any(Number),
			launcherParentDevice: expect.any(Number),
			layout: "npm-posix-product-tree",
		});
		expect(Object.isFrozen(command?.npmRecovery)).toBe(true);
		expect(readlinkSync(fixture.launcherPath)).toBe(fixture.launcherLinkText);
	});

	it("qualifies the pre-populated layout when the runtime still resolves inside the product tree", async () => {
		createNpmFixture("prepopulated");
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery?.layout).toBe("npm-posix-product-tree");
	});

	it("rejects a pre-populated layout when the executing runtime is hoisted outside the product tree", async () => {
		createNpmFixture("hoisted-only");
		const { getSelfUpdateCommand } = await import("../src/config.js");

		const command = getSelfUpdateCommand("@leanandmean/scramjet");

		expect(command?.args.slice(-3)).toEqual(["install", "-g", "@leanandmean/scramjet"]);
		expect(command?.npmRecovery).toBeUndefined();
	});

	it("leaves the ordinary command unqualified without launcher-derived product identity", async () => {
		createNpmFixture();
		delete process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT];
		const { getSelfUpdateCommand } = await import("../src/config.js");

		const command = getSelfUpdateCommand("@leanandmean/scramjet");

		expect(command?.args.slice(-3)).toEqual(["install", "-g", "@leanandmean/scramjet"]);
		expect(command?.npmRecovery).toBeUndefined();
	});

	it("does not qualify package-name migration", async () => {
		createNpmFixture();
		const { getSelfUpdateCommand } = await import("../src/config.js");

		const command = getSelfUpdateCommand("@leanandmean/scramjet", undefined, "@leanandmean/replacement");

		expect(command?.npmRecovery).toBeUndefined();
	});

	it("does not qualify an unproved configured npm wrapper", async () => {
		createNpmFixture();
		const { getSelfUpdateCommand } = await import("../src/config.js");

		const command = getSelfUpdateCommand("@leanandmean/scramjet", ["npm", "--prefix", "/custom"]);

		expect(command?.npmRecovery).toBeUndefined();
	});

	it("rejects a product root that is not the exact package path under npm root", async () => {
		const fixture = createNpmFixture();
		process.env[SCRAMJET_INTERNAL_PRODUCT_ROOT] = fixture.codingAgentRoot;
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects mismatched package and bin identity", async () => {
		const fixture = createNpmFixture();
		writeFileSync(
			fixture.manifestPath,
			JSON.stringify({ name: "@leanandmean/other", bin: { scramjet: "./wrong.js" } }),
		);
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects a launcher with noncanonical link text", async () => {
		const fixture = createNpmFixture();
		const alternateLink = "../lib/node_modules/@leanandmean/scramjet/bin/../bin/scramjet.js";
		unlinkSync(fixture.launcherPath);
		symlinkSync(alternateLink, fixture.launcherPath);
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects a launcher that is not a symlink", async () => {
		const fixture = createNpmFixture();
		unlinkSync(fixture.launcherPath);
		writeFileSync(fixture.launcherPath, "not an npm launcher\n");
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects a bin target that escapes the product root", async () => {
		const fixture = createNpmFixture();
		const escapedTarget = join(fixture.fixture, "outside.js");
		writeFileSync(escapedTarget, "#!/usr/bin/env node\n");
		writeFileSync(
			fixture.manifestPath,
			JSON.stringify({
				name: "@leanandmean/scramjet",
				bin: { scramjet: relative(fixture.productRoot, escapedTarget) },
			}),
		);
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects unsupported native Windows qualification", async () => {
		createNpmFixture();
		vi.spyOn(process, "platform", "get").mockReturnValue("win32");
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
	});

	it("rejects qualification when a transaction path is not writable", async () => {
		const fixture = createNpmFixture();
		vi.doMock("fs", async (importOriginal) => ({
			...(await importOriginal<typeof import("fs")>()),
			accessSync: vi.fn((path: string) => {
				if (path === dirname(fixture.launcherPath)) throw new Error("unwritable launcher parent");
			}),
		}));
		const { getSelfUpdateCommand } = await import("../src/config.js");

		expect(getSelfUpdateCommand("@leanandmean/scramjet")?.npmRecovery).toBeUndefined();
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
