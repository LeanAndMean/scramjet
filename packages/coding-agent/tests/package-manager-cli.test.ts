import { EventEmitter } from "node:events";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
	command: undefined as
		| { command: string; args: string[]; display: string; npmRecovery?: Record<string, unknown> }
		| undefined,
	outcome: undefined as unknown,
	transactionError: undefined as Error | undefined,
	packageManager: {
		installAndPersist: vi.fn(),
		removeAndPersist: vi.fn(),
		listConfiguredPackages: vi.fn(() => []),
		update: vi.fn(),
		setProgressCallback: vi.fn(),
		resolve: vi.fn(),
	},
	settingsManager: {
		drainErrors: vi.fn(() => []),
		getGlobalSettings: vi.fn(() => ({})),
	},
	spawn: vi.fn(),
	runTransaction: vi.fn(),
}));

vi.mock("../src/config.js", () => ({
	APP_NAME: "scramjet",
	PACKAGE_NAME: "@leanandmean/scramjet",
	VERSION: "1.0.0",
	getAgentDir: () => "/agent",
	getSelfUpdateCommand: () => mocks.command,
	getSelfUpdateUnavailableInstruction: () => "install manually",
}));
vi.mock("../src/core/package-manager.js", () => ({
	DefaultPackageManager: class {
		installAndPersist = mocks.packageManager.installAndPersist;
		removeAndPersist = mocks.packageManager.removeAndPersist;
		listConfiguredPackages = mocks.packageManager.listConfiguredPackages;
		update = mocks.packageManager.update;
		setProgressCallback = mocks.packageManager.setProgressCallback;
		resolve = mocks.packageManager.resolve;
	},
}));
vi.mock("../src/core/settings-manager.js", () => ({
	SettingsManager: { create: () => mocks.settingsManager },
}));
vi.mock("../src/cli/config-selector.js", () => ({ selectConfig: vi.fn() }));
vi.mock("../src/utils/version-check.js", () => ({
	getLatestRelease: vi.fn(),
	isNewerPackageVersion: vi.fn(),
}));
vi.mock("child_process", () => ({ spawn: mocks.spawn }));
vi.mock("../src/npm-self-update-transaction.js", () => ({
	runNpmSelfUpdateTransaction: mocks.runTransaction,
}));

import { handlePackageCommand } from "../src/package-manager-cli.js";

const recovery = { layout: "npm-posix-product-tree" };

function failure(phase: string, message: string, path?: string) {
	return { phase, error: new Error(message), ...(path ? { path } : {}) };
}

async function runUpdate(
	args = ["update", "--self", "--force"],
): Promise<{ stdout: string[]; stderr: string[]; events: string[] }> {
	const stdout: string[] = [];
	const stderr: string[] = [];
	const events: string[] = [];
	vi.spyOn(console, "log").mockImplementation((value) => {
		stdout.push(String(value));
		events.push(`stdout: ${value}`);
	});
	vi.spyOn(console, "error").mockImplementation((value) => {
		stderr.push(String(value));
		events.push(`stderr: ${value}`);
	});
	expect(await handlePackageCommand(args)).toBe(true);
	return { stdout, stderr, events };
}

beforeEach(() => {
	process.exitCode = undefined;
	mocks.command = {
		command: "npm",
		args: ["install", "-g", "@leanandmean/scramjet@latest"],
		display: "npm install -g @leanandmean/scramjet@latest",
		npmRecovery: recovery,
	};
	mocks.outcome = { status: "committed", cleanupFailures: [], retainedPaths: [] };
	mocks.transactionError = undefined;
	mocks.runTransaction.mockImplementation(async () => {
		if (mocks.transactionError) throw mocks.transactionError;
		return mocks.outcome;
	});
	mocks.packageManager.update.mockReset().mockResolvedValue(undefined);
	mocks.packageManager.setProgressCallback.mockReset();
	mocks.settingsManager.drainErrors.mockReturnValue([]);
	mocks.settingsManager.getGlobalSettings.mockReturnValue({});
	mocks.spawn.mockReset();
});

afterEach(() => {
	vi.restoreAllMocks();
	process.exitCode = undefined;
});

describe("qualified npm self-update reporting", () => {
	it("prints success only after a verified commit", async () => {
		const output = await runUpdate();

		expect(output.stdout).toEqual([
			"Updating scramjet with npm install -g @leanandmean/scramjet@latest...",
			"Updated scramjet",
		]);
		expect(output.stderr).toEqual([]);
		expect(process.exitCode).toBeUndefined();
	});

	it("keeps a verified commit successful while reporting incomplete cleanup", async () => {
		mocks.outcome = {
			status: "committed-with-retained-artifacts",
			cleanupFailures: [failure("cleanup", "directory not empty", "/prefix/.backup")],
			retainedPaths: ["/prefix/.backup"],
		};

		const output = await runUpdate();

		expect(output.stdout.at(-1)).toBe("Updated scramjet");
		expect(output.stderr).toEqual([
			"The updated launcher and package runtime were verified, but cleanup was incomplete.",
			"Warning: update artifact retained at /prefix/.backup",
			"Warning: cleanup failed during cleanup: directory not empty (/prefix/.backup)",
		]);
		expect(process.exitCode).toBeUndefined();
	});

	it("preserves extension-first output and reports verified runtime restoration", async () => {
		mocks.outcome = {
			status: "restored",
			updateFailure: failure("update-process", "npm exited with code 1"),
			cleanupFailures: [failure("cleanup", "quarantine busy", "/prefix/.quarantine")],
			retainedPaths: ["/prefix/.quarantine"],
		};

		const output = await runUpdate(["update", "--force"]);

		expect(output.events).toEqual([
			"stdout: Updated packages",
			"stdout: Updating scramjet with npm install -g @leanandmean/scramjet@latest...",
			"stderr: Self-update failed during update-process: npm exited with code 1",
			"stderr: The previous launcher and package runtime were restored and verified.",
			"stderr: Postinstall-managed command data may have changed.",
			"stderr: Warning: update artifact retained at /prefix/.quarantine",
			"stderr: Warning: cleanup failed during cleanup: quarantine busy (/prefix/.quarantine)",
		]);
		expect(output.stderr).toEqual([
			"Self-update failed during update-process: npm exited with code 1",
			"The previous launcher and package runtime were restored and verified.",
			"Postinstall-managed command data may have changed.",
			"Warning: update artifact retained at /prefix/.quarantine",
			"Warning: cleanup failed during cleanup: quarantine busy (/prefix/.quarantine)",
		]);
		expect(output.stdout).not.toContain("Updated scramjet");
		expect(process.exitCode).toBe(1);
	});

	it("reports unverified restoration without an availability claim or retry fallback", async () => {
		mocks.outcome = {
			status: "restoration-unverified",
			updateFailure: failure("replacement-probe", "probe failed"),
			restorationFailures: [failure("restored-structure", "launcher target changed", "/prefix/bin/scramjet")],
			retainedPaths: ["/prefix/.backup", "/prefix/.quarantine"],
		};

		const output = await runUpdate();

		expect(output.stderr).toEqual([
			"Self-update failed during replacement-probe: probe failed",
			"Restoration failed during restored-structure: launcher target changed (/prefix/bin/scramjet)",
			"Warning: update artifact retained at /prefix/.backup",
			"Warning: update artifact retained at /prefix/.quarantine",
			"The previous launcher and package runtime could not be verified.",
			"Inspect any retained artifacts before manually repairing the installation; do not repeat the update yet.",
		]);
		expect(output.stderr.join("\n")).not.toContain("restored and verified");
		expect(output.stderr.join("\n")).not.toContain("run this command yourself");
		expect(output.stdout).not.toContain("Updated scramjet");
		expect(process.exitCode).toBe(1);
	});

	it("does not recommend repeating a qualified update when transaction loading fails", async () => {
		mocks.transactionError = new Error("transaction unavailable");

		const output = await runUpdate();

		expect(output.stderr).toEqual([
			"Error: transaction unavailable",
			"The recovery transaction did not complete; the launcher and package runtime were not verified.",
		]);
		expect(process.exitCode).toBe(1);
	});
});

describe("unqualified self-update compatibility", () => {
	it.each([
		["npm", ["install", "-g", "@leanandmean/scramjet@latest"]],
		["pnpm", ["install", "-g", "@leanandmean/scramjet@latest"]],
		["yarn", ["global", "add", "@leanandmean/scramjet@latest"]],
		["bun", ["install", "-g", "@leanandmean/scramjet@latest"]],
	])("preserves the existing %s command, inherited stdio, and fallback", async (manager, args) => {
		const display = [manager, ...args].join(" ");
		mocks.command = { command: manager, args, display };
		mocks.spawn.mockImplementation(() => {
			const child = new EventEmitter();
			queueMicrotask(() => child.emit("close", 1, null));
			return child;
		});

		const output = await runUpdate();

		expect(mocks.runTransaction).not.toHaveBeenCalled();
		expect(mocks.spawn).toHaveBeenCalledWith(manager, args, { stdio: "inherit", shell: false });
		expect(output.stderr).toEqual([
			`Error: ${display} exited with code 1`,
			`If this keeps failing, run this command yourself: ${display}`,
		]);
		expect(process.exitCode).toBe(1);
	});
});
