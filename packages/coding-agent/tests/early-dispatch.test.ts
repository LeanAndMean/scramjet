import { afterEach, describe, expect, it, vi } from "vitest";

const originalScramjetOffline = process.env.SCRAMJET_OFFLINE;
const originalPiOffline = process.env.PI_OFFLINE;
const originalSkipVersionCheck = process.env.PI_SKIP_VERSION_CHECK;
const originalDisplay = process.env.DISPLAY;

function restoreEnv(name: string, value: string | undefined): void {
	if (value === undefined) delete process.env[name];
	else process.env[name] = value;
}

afterEach(() => {
	restoreEnv("SCRAMJET_OFFLINE", originalScramjetOffline);
	restoreEnv("PI_OFFLINE", originalPiOffline);
	restoreEnv("PI_SKIP_VERSION_CHECK", originalSkipVersionCheck);
	restoreEnv("DISPLAY", originalDisplay);
	vi.restoreAllMocks();
	vi.doUnmock("../src/package-manager-cli.js");
	vi.doUnmock("@mariozechner/clipboard");
	vi.resetModules();
});

describe("dispatchEarlyCliCommand", () => {
	it("does not load clipboard-native while importing the real early path", async () => {
		process.env.DISPLAY = ":early-dispatch-boundary";
		vi.doMock("@mariozechner/clipboard", () => {
			throw new Error("clipboard native loaded during early dispatch");
		});

		await expect(import("../src/early-dispatch.js")).resolves.toHaveProperty("dispatchEarlyCliCommand");
	});

	it("normalizes offline state before package routing", async () => {
		delete process.env.SCRAMJET_OFFLINE;
		delete process.env.PI_OFFLINE;
		delete process.env.PI_SKIP_VERSION_CHECK;
		const handlePackageCommand = vi.fn(() => {
			expect(process.env.SCRAMJET_OFFLINE).toBe("1");
			expect(process.env.PI_OFFLINE).toBe("1");
			expect(process.env.PI_SKIP_VERSION_CHECK).toBe("1");
			return Promise.resolve(true);
		});
		const handleConfigCommand = vi.fn(() => Promise.resolve(false));
		vi.doMock("../src/package-manager-cli.js", () => ({ handlePackageCommand, handleConfigCommand }));
		const { dispatchEarlyCliCommand } = await import("../src/early-dispatch.js");

		expect(await dispatchEarlyCliCommand(["update", "--offline"])).toBe(true);
		expect(handlePackageCommand).toHaveBeenCalledWith(["update", "--offline"]);
		expect(handleConfigCommand).not.toHaveBeenCalled();
	});

	it("preserves package-before-config routing and returns the config result", async () => {
		const order: string[] = [];
		const handlePackageCommand = vi.fn(async () => {
			order.push("package");
			return false;
		});
		const handleConfigCommand = vi.fn(async () => {
			order.push("config");
			return true;
		});
		vi.doMock("../src/package-manager-cli.js", () => ({ handlePackageCommand, handleConfigCommand }));
		const { dispatchEarlyCliCommand } = await import("../src/early-dispatch.js");

		expect(await dispatchEarlyCliCommand(["config"])).toBe(true);
		expect(order).toEqual(["package", "config"]);
	});
});
