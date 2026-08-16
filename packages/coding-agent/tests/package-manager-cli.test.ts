import { EventEmitter } from "events";
import { mkdirSync, mkdtempSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { dirname, join } from "path";
import { afterEach, describe, expect, it, vi } from "vitest";

const PACKAGE_NAME = "@leanandmean/scramjet";
const CURRENT_VERSION = "0.77.1";
const RELEASE_VERSION = "0.78.1";

interface RunOptions {
	args?: string[];
	manifest?: unknown;
	missingManifest?: boolean;
	releaseVersion?: string;
	releaseError?: Error;
	npmCommand?: string[];
	afterSpawn?: (targetManifestPath: string) => void;
}

async function runSelfUpdate(options: RunOptions = {}) {
	const fixture = mkdtempSync(join(tmpdir(), "scramjet-package-manager-cli-"));
	const targetManifestPath = join(fixture, "node_modules", "@leanandmean", "scramjet", "package.json");
	mkdirSync(dirname(targetManifestPath), { recursive: true });
	if (!options.missingManifest) {
		const manifest = options.manifest ?? { name: PACKAGE_NAME, version: CURRENT_VERSION };
		writeFileSync(targetManifestPath, typeof manifest === "string" ? manifest : JSON.stringify(manifest));
	}

	const releaseVersion = options.releaseVersion ?? RELEASE_VERSION;
	const display = `npm install -g ${PACKAGE_NAME}@${releaseVersion}`;
	const spawn = vi.fn(() => {
		const child = new EventEmitter();
		queueMicrotask(() => {
			options.afterSpawn?.(targetManifestPath);
			child.emit("close", 0, null);
		});
		return child;
	});
	const update = vi.fn(async () => {});
	const stdout = vi.spyOn(console, "log").mockImplementation(() => {});
	const stderr = vi.spyOn(console, "error").mockImplementation(() => {});
	process.exitCode = undefined;

	vi.doMock("child_process", () => ({ spawn }));
	vi.doMock("../src/config.js", () => ({
		APP_NAME: "scramjet",
		getAgentDir: () => fixture,
		getSelfUpdateCommand: (
			_packageName: string,
			_updatePackageName: string,
			installSpec: string,
			npmCommand?: string[],
		) => {
			const [command = "npm", ...prefixArgs] = npmCommand ?? [];
			return {
				command,
				args: [...prefixArgs, "install", "-g", installSpec],
				display,
				targetManifestPath,
			};
		},
		getSelfUpdateUnavailableInstruction: vi.fn(),
		PACKAGE_NAME,
		VERSION: CURRENT_VERSION,
	}));
	vi.doMock("../src/core/settings-manager.js", () => ({
		SettingsManager: {
			create: () => ({
				drainErrors: () => [],
				getGlobalSettings: () => ({ npmCommand: options.npmCommand }),
			}),
		},
	}));
	vi.doMock("../src/core/package-manager.js", () => ({
		DefaultPackageManager: class {
			setProgressCallback() {}
			update = update;
		},
	}));
	vi.doMock("../src/utils/child-process.js", () => ({ shouldUseWindowsShell: () => false }));
	const resolveCurrentRelease = vi.fn(async () => {
		if (options.releaseError) throw options.releaseError;
		return { packageName: PACKAGE_NAME, version: releaseVersion };
	});
	vi.doMock("../src/utils/version-check.js", () => ({
		isNewerPackageVersion: (candidate: string, current: string) =>
			candidate.localeCompare(current, undefined, { numeric: true }) > 0,
		resolveCurrentRelease,
	}));

	const { handlePackageCommand } = await import("../src/package-manager-cli.js");
	const handled = await handlePackageCommand(options.args ?? ["update", "--self"]);
	return {
		display,
		handled,
		spawn,
		resolveCurrentRelease,
		stderr: stderr.mock.calls.flat().join("\n"),
		stdout: stdout.mock.calls.flat().join("\n"),
		targetManifestPath,
		update,
	};
}

afterEach(() => {
	process.exitCode = undefined;
	vi.restoreAllMocks();
	vi.resetModules();
	vi.clearAllMocks();
});

describe("managed self-update verification", () => {
	it("fails when the package manager exits successfully but leaves the old release installed", async () => {
		const result = await runSelfUpdate();

		expect(result.handled).toBe(true);
		expect(result.spawn).toHaveBeenCalledOnce();
		expect(process.exitCode).toBe(1);
		expect(result.stderr).toContain(`${PACKAGE_NAME}@${RELEASE_VERSION}`);
		expect(result.stderr).toContain(result.targetManifestPath);
		expect(result.stderr).toContain(`version "${CURRENT_VERSION}"`);
		expect(result.stderr).toContain(result.display);
		expect(result.stdout).not.toContain("Updated scramjet");
	});

	it("reports success only after the managed manifest matches the release", async () => {
		const result = await runSelfUpdate({
			afterSpawn: (path) => writeFileSync(path, JSON.stringify({ name: PACKAGE_NAME, version: RELEASE_VERSION })),
		});

		expect(process.exitCode).toBeUndefined();
		expect(result.stderr).toBe("");
		expect(result.stdout).toContain("Updated scramjet");
	});

	it.each([
		["wrong name", { name: "@other/package", version: RELEASE_VERSION }, "@other/package"],
		["malformed metadata", "not json", "could not read valid package metadata"],
		["non-object metadata", [], "manifest is not a JSON object"],
	])("fails for %s", async (_name, manifest, expectedDetail) => {
		const result = await runSelfUpdate({ manifest });

		expect(process.exitCode).toBe(1);
		expect(result.stderr).toContain(expectedDetail);
		expect(result.stderr).toContain(result.targetManifestPath);
		expect(result.stderr).toContain(result.display);
		expect(result.stdout).not.toContain("Updated scramjet");
	});

	it("fails when the managed manifest is missing", async () => {
		const result = await runSelfUpdate({ missingManifest: true });

		expect(process.exitCode).toBe(1);
		expect(result.stderr).toContain("could not read valid package metadata");
		expect(result.stderr).toContain(result.targetManifestPath);
		expect(result.stdout).not.toContain("Updated scramjet");
	});

	it("force still resolves, installs, and verifies the exact release", async () => {
		const result = await runSelfUpdate({
			args: ["update", "--self", "--force"],
			releaseVersion: CURRENT_VERSION,
		});

		expect(result.spawn).toHaveBeenCalledOnce();
		expect(process.exitCode).toBeUndefined();
		expect(result.stdout).toContain("Updated scramjet");
	});

	it("skips installation when the current release is already installed", async () => {
		const result = await runSelfUpdate({ releaseVersion: CURRENT_VERSION });

		expect(result.spawn).not.toHaveBeenCalled();
		expect(result.stdout).toContain("already up to date");
	});

	it("uses the configured npm command for release lookup and installation", async () => {
		const npmCommand = ["mise", "exec", "node@20", "--", "npm"];
		const result = await runSelfUpdate({
			npmCommand,
			afterSpawn: (path) => writeFileSync(path, JSON.stringify({ name: PACKAGE_NAME, version: RELEASE_VERSION })),
		});

		expect(result.resolveCurrentRelease).toHaveBeenCalledWith(PACKAGE_NAME, undefined, undefined, npmCommand);
		expect(result.spawn).toHaveBeenCalledWith(
			"mise",
			["exec", "node@20", "--", "npm", "install", "-g", `${PACKAGE_NAME}@${RELEASE_VERSION}`],
			{ stdio: "inherit", shell: false },
		);
	});

	it("reports release lookup failure without spawning or claiming success", async () => {
		const result = await runSelfUpdate({ releaseError: new Error("registry unavailable") });

		expect(process.exitCode).toBe(1);
		expect(result.spawn).not.toHaveBeenCalled();
		expect(result.stderr).toContain("registry unavailable");
		expect(result.stdout).not.toContain("Updated scramjet");
	});

	it("does not downgrade when the registry release is older", async () => {
		const olderVersion = "0.76.9";
		const result = await runSelfUpdate({ releaseVersion: olderVersion });

		expect(result.spawn).not.toHaveBeenCalled();
		expect(result.stdout).toContain(`v${CURRENT_VERSION}`);
		expect(result.stdout).toContain(`v${olderVersion}`);
		expect(result.stdout).toContain("not downgrading");
	});

	it("updates extensions first and returns nonzero when self-update verification fails", async () => {
		const result = await runSelfUpdate({ args: ["update"] });

		expect(result.update).toHaveBeenCalledOnce();
		expect(process.exitCode).toBe(1);
		expect(result.stdout).toContain("Updated packages");
		expect(result.stdout).not.toContain("Updated scramjet");
	});
});
