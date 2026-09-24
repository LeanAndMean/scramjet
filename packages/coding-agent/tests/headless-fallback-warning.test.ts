import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

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
vi.mock("../src/core/output-guard.js", () => ({
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

	it("writes a successful fallback warning to stderr without stdout contamination", async () => {
		root = mkdtempSync(join(tmpdir(), "headless-default-fallback-"));
		process.chdir(root);
		process.env.SCRAMJET_CODING_AGENT_DIR = join(root, "agent");
		process.env.SCRAMJET_CODING_AGENT_SESSION_DIR = join(root, "sessions");
		Object.defineProperty(process.stdin, "isTTY", { configurable: true, value: true });
		const stderr = vi.spyOn(console, "error").mockImplementation(() => {});
		const stdout = vi.spyOn(console, "log").mockImplementation(() => {});

		const { main } = await import("../src/main.js");
		await main(["--offline", "--print", "hello"]);

		expect(stderr.mock.calls.map((call) => String(call[0])).join("\n")).toContain(`Warning: ${fallbackMessage}`);
		expect(stdout.mock.calls.flat().join("\n")).not.toContain(fallbackMessage);
	});
});
