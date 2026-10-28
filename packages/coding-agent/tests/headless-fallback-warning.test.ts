import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsManager } from "../src/core/settings-manager.js";

const fallbackMessage =
	"Configured default model cerebras/missing-default is not in the model registry. Using cerebras/gpt-oss-120b.";

vi.mock("../src/core/agent-session-runtime.js", () => ({
	createAgentSessionRuntime: vi.fn(async () => ({
		services: {
			settingsManager: {
				drainErrors: () => [],
				getImageAutoResize: () => false,
				getTheme: () => undefined,
			},
			modelRegistry: {},
			resourceLoader: { getExtensions: () => ({ extensions: [], errors: [] }) },
		},
		session: { model: { provider: "cerebras", id: "gpt-oss-120b" } },
		modelFallbackMessage: fallbackMessage,
		diagnostics: [],
	})),
}));
vi.mock("../src/core/output-guard.js", async (importOriginal) => ({
	...(await importOriginal<typeof import("../src/core/output-guard.js")>()),
	restoreStdout: vi.fn(),
	takeOverStdout: vi.fn(),
}));
vi.mock("../src/modes/index.js", () => ({
	InteractiveMode: class {},
	runPrintMode: vi.fn(async () => 0),
	runRpcMode: vi.fn(async () => {}),
}));
vi.mock("../src/modes/interactive/theme/theme.js", () => ({
	initTheme: vi.fn(),
	stopThemeWatcher: vi.fn(),
}));

describe("headless configured-default fallback", () => {
	const originalCwd = process.cwd();
	const originalStdinIsTTY = Object.getOwnPropertyDescriptor(process.stdin, "isTTY");
	let root: string | undefined;

	afterEach(() => {
		process.chdir(originalCwd);
		if (root) rmSync(root, { recursive: true, force: true });
		root = undefined;
		delete process.env.SCRAMJET_OFFLINE;
		delete process.env.PI_OFFLINE;
		delete process.env.PI_SKIP_VERSION_CHECK;
		delete process.env.SCRAMJET_CODING_AGENT_DIR;
		delete process.env.SCRAMJET_CODING_AGENT_SESSION_DIR;
		if (originalStdinIsTTY) Object.defineProperty(process.stdin, "isTTY", originalStdinIsTTY);
		else delete (process.stdin as NodeJS.ReadStream & { isTTY?: boolean }).isTTY;
		vi.restoreAllMocks();
	});

	it("writes fallback and full startup settings diagnostics to stderr without stdout contamination", async () => {
		root = mkdtempSync(join(tmpdir(), "headless-default-fallback-"));
		process.chdir(root);
		process.env.SCRAMJET_CODING_AGENT_DIR = join(root, "agent");
		process.env.SCRAMJET_CODING_AGENT_SESSION_DIR = join(root, "sessions");
		Object.defineProperty(process.stdin, "isTTY", { configurable: true, value: true });
		const stderr = vi.spyOn(console, "error").mockImplementation(() => {});
		const stdout = vi.spyOn(console, "log").mockImplementation(() => {});

		const reason = Object.assign(new Error("nested startup settings reason"), { code: "E_STARTUP_SETTINGS" });
		reason.stack = "Error: nested startup settings reason\n    at startup-settings-evidence";
		const error = new Error("startup settings failed\x1b[2J\x07", { cause: { reason } });
		const settings = SettingsManager.inMemory();
		vi.spyOn(settings, "drainErrors").mockReturnValueOnce([{ scope: "global", error }]);
		vi.spyOn(SettingsManager, "create").mockReturnValueOnce(settings);

		const { main } = await import("../src/main.js");
		await main(["--offline", "--print", "hello"]);

		expect(stderr.mock.calls.map((call) => String(call[0])).join("\n")).toContain(`Warning: ${fallbackMessage}`);
		const diagnostic = stderr.mock.calls
			.map((call) => String(call[0]))
			.find((text) => text.includes("startup session lookup"));
		expect(diagnostic).toContain("Settings persistence (global)");
		for (const detail of ["nested startup settings reason", "E_STARTUP_SETTINGS", "startup-settings-evidence"]) {
			expect(diagnostic).toContain(detail);
			expect(stdout.mock.calls.flat().join("\n")).not.toContain(detail);
		}
		expect(diagnostic).not.toMatch(/[\x00-\x1f\x7f-\x9f]/);
		expect(stdout.mock.calls.flat().join("\n")).not.toContain(fallbackMessage);
	});
});
