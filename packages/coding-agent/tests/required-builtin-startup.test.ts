import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentSessionServices } from "../src/core/agent-session-services.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { RequiredBuiltinInitError } from "../src/core/resource-loader.js";
import { SettingsManager } from "../src/core/settings-manager.js";

function servicesOptions(resourceLoaderOptions: Record<string, unknown>) {
	const dir = mkdtempSync(join(tmpdir(), "required-builtin-startup-"));
	const authStorage = AuthStorage.inMemory();
	return {
		cwd: join(dir, "cwd"),
		agentDir: join(dir, "agent"),
		authStorage,
		settingsManager: SettingsManager.inMemory(),
		modelRegistry: ModelRegistry.inMemory(authStorage),
		resourceLoaderOptions,
	};
}

describe("candidate preparation fails fast before AgentSession/session_start", () => {
	it("rejects with RequiredBuiltinInitError when the required builtin throws synchronously", async () => {
		const original = new Error("sync kaboom");
		await expect(
			createAgentSessionServices(
				servicesOptions({
					builtinInit: () => {
						throw original;
					},
				}),
			),
		).rejects.toBeInstanceOf(RequiredBuiltinInitError);
	});

	it("rejects with RequiredBuiltinInitError when the required builtin rejects asynchronously", async () => {
		const original = new Error("async kaboom");
		let caught: unknown;
		try {
			await createAgentSessionServices(
				servicesOptions({
					builtinInit: async () => {
						throw original;
					},
				}),
			);
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(RequiredBuiltinInitError);
		expect((caught as Error).cause).toBe(original);
	});

	it("does not convert an optional extension failure into a required-builtin throw", async () => {
		const missing = join(mkdtempSync(join(tmpdir(), "required-builtin-startup-missing-")), "missing-extension.ts");
		const services = await createAgentSessionServices(
			servicesOptions({
				additionalExtensionPaths: [missing],
			}),
		);
		const errors = services.resourceLoader.getExtensions().errors;
		expect(errors.some((e) => e.path.includes("missing-extension"))).toBe(true);
	});
});

describe("main() surfaces the required builtin failure at startup", () => {
	const originalCwd = process.cwd();

	afterEach(async () => {
		process.chdir(originalCwd);
		const { restoreStdout } = await import("../src/core/output-guard.js");
		restoreStdout();
		vi.restoreAllMocks();
		const { ENV_AGENT_DIR, ENV_SESSION_DIR } = await import("../src/config.js");
		delete process.env[ENV_AGENT_DIR];
		delete process.env[ENV_SESSION_DIR];
	});

	it("exits non-zero with a Scramjet-attributed error and retains the cause", async () => {
		const { ENV_AGENT_DIR, ENV_SESSION_DIR } = await import("../src/config.js");
		const dir = mkdtempSync(join(tmpdir(), "required-builtin-main-"));
		const agentDir = join(dir, "agent");
		process.env[ENV_AGENT_DIR] = agentDir;
		process.env[ENV_SESSION_DIR] = join(agentDir, "sessions");
		process.chdir(dir);

		class ExitSignal extends Error {
			constructor(readonly code: number | undefined) {
				super(`process.exit(${code})`);
			}
		}
		const exitSpy = vi.spyOn(process, "exit").mockImplementation(((code?: number) => {
			throw new ExitSignal(code);
		}) as never);
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

		const { main } = await import("../src/main.js");
		const original = new Error("builtin boom detail");

		let caught: unknown;
		try {
			await main(["--offline"], {
				builtinInit: () => {
					throw original;
				},
			});
		} catch (error) {
			caught = error;
		}

		expect(caught).toBeInstanceOf(ExitSignal);
		expect((caught as ExitSignal).code).toBe(1);
		expect(exitSpy).toHaveBeenCalledWith(1);

		const output = errorSpy.mock.calls.map((call) => String(call[0])).join("\n");
		expect(output).toMatch(/scramjet/i);
		expect(output).toContain("builtin boom detail");
	});
});
