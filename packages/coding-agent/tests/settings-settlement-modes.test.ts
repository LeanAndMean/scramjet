import { existsSync } from "node:fs";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { inspect, stripVTControlCharacters } from "node:util";
import { afterEach, describe, expect, it, vi } from "vitest";
import { InMemoryLastUsedModelsStorage } from "../src/core/last-used-models.js";
import { formatSettingsError } from "../src/core/output-guard.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { runPrintMode } from "../src/modes/print-mode.js";
import { runRpcMode } from "../src/modes/rpc/rpc-mode.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

const output = vi.hoisted(() => ({ write: vi.fn(), stderrFlush: vi.fn(async () => {}) }));
vi.mock("../src/core/output-guard.js", async (importOriginal) => ({
	...(await importOriginal<typeof import("../src/core/output-guard.js")>()),
	takeOverStdout: vi.fn(),
	writeRawStdout: output.write,
	flushStderr: output.stderrFlush,
}));
vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));
afterEach(() => {
	vi.restoreAllMocks();
	output.write.mockClear();
	output.stderrFlush.mockClear();
});

function preferences() {
	const recall = new InMemoryLastUsedModelsStorage();
	vi.spyOn(recall, "withLock").mockRejectedValue(new Error("write refused"));
	return SettingsManager.inMemory({ theme: "pi-dark", quietStartup: true, compaction: { enabled: false } }, recall);
}

describe("mode-owned settings settlement", () => {
	it("retains complete nested diagnostics while neutralizing terminal controls", () => {
		const original = Object.assign(new Error("original failure", { cause: new SyntaxError("parse detail") }), {
			code: "ECUSTOM",
		});
		const longValue = `${"x".repeat(12000)}STRING_END`;
		const values = Array.from({ length: 150 }, (_, index) => `item-${index}`);
		const nested = { one: { two: { three: { four: { five: { longValue, values } } } } } };
		const error = new Error("context\u001b[31munsafe\u001b[0m\n\u0007\u009b", {
			cause: new AggregateError([original, nested], "multiple failures"),
		});
		const text = formatSettingsError({ scope: "global", error });
		const diagnostic = inspect(error, {
			depth: null,
			colors: false,
			customInspect: false,
			getters: false,
			maxStringLength: null,
			maxArrayLength: null,
		});
		expect(text).toContain(stripVTControlCharacters(diagnostic).replace(/\p{Cc}/gu, " "));
		expect(text).toContain("unsafe");
		expect(text).toContain("ECUSTOM");
		expect(text).toContain("parse detail");
		expect(text).toContain("STRING_END");
		expect(text).toContain("item-149");
		expect(text).toContain(original.stack!.split("\n")[1].trim());
		expect(text).not.toMatch(/\p{Cc}/u);
		expect(text.length).toBeGreaterThan(12000);
	});

	it("does not execute custom inspectors or getters while formatting arbitrary causes", () => {
		const customInspect = vi.fn(() => "CUSTOM_INSPECTOR_OUTPUT");
		const getter = vi.fn(() => "GETTER_OUTPUT");
		const cause = Object.defineProperty({ detail: "original detail", [inspect.custom]: customInspect }, "value", {
			enumerable: true,
			get: getter,
		});
		const text = formatSettingsError({ scope: "project", error: new Error("context", { cause }) });
		expect(text).toContain("original detail");
		expect(text).toContain("[Getter]");
		expect(text).not.toContain("CUSTOM_INSPECTOR_OUTPUT");
		expect(text).not.toContain("GETTER_OUTPUT");
		expect(customInspect).not.toHaveBeenCalled();
		expect(getter).not.toHaveBeenCalled();
	});

	it.each(["text", "json"] as const)(
		"print %s settles shutdown selections and reports only on stderr",
		async (mode) => {
			const settings = preferences();
			const h = await createProductionInteractiveHarness(
				60,
				24,
				(pi) => {
					pi.on("session_shutdown", () => {
						settings.setDefaultModelAndProvider("p", "m", "/a");
					});
				},
				true,
				settings,
			);
			const error = vi.spyOn(console, "error").mockImplementation(() => {});
			try {
				expect(await runPrintMode(h.runtime, { mode })).toBe(0);
				expect(error.mock.calls.flat().join(" ")).toContain("Last-used models");
				expect(output.write.mock.calls.flat().join(" ")).not.toContain("Last-used models");
				expect(output.stderrFlush).toHaveBeenCalledOnce();
				expect(settings.drainErrors()).toEqual([]);
				expect(existsSync(join(h.runtime.services.agentDir, "last-used-models.json"))).toBe(false);
			} finally {
				await h.dispose();
			}
		},
	);

	it("interactive reload warns actively, and final shutdown reports safely after terminal release", async () => {
		const settings = preferences();
		const h = await createProductionInteractiveHarness(
			60,
			24,
			(pi) => {
				pi.on("session_shutdown", () => {
					settings.setDefaultModelAndProvider("p", "m", "/a");
				});
			},
			true,
			settings,
		);
		const warning = vi.spyOn(h.mode, "showWarning");
		const error = vi.spyOn(console, "error").mockImplementation(() => {});
		const exit = vi.spyOn(process, "exit").mockImplementation(() => undefined as never);
		try {
			settings.applyOverrides({ retry: { maxRetries: -1 } });
			await h.internals.handleReloadCommand();
			expect(warning.mock.calls.flat().join(" ")).toContain("retry.maxRetries");
			expect(error).not.toHaveBeenCalled();
			const stop = vi.spyOn(h.mode, "stop");
			await (h.mode as unknown as { shutdown(): Promise<void> }).shutdown();
			expect(error.mock.calls.flat().join(" ")).toContain("Last-used models");
			expect(stop.mock.invocationCallOrder[0]).toBeLessThan(error.mock.invocationCallOrder[0]);
			expect(output.stderrFlush.mock.invocationCallOrder[0]).toBeLessThan(exit.mock.invocationCallOrder[0]);
			expect(exit).toHaveBeenCalledWith(0);
		} finally {
			await h.dispose();
		}
	});

	it("RPC EOF settles shutdown selections and keeps settings errors out of protocol stdout", async () => {
		const settings = preferences();
		const h = await createProductionInteractiveHarness(
			60,
			24,
			(pi) => {
				pi.on("session_shutdown", () => {
					settings.setDefaultModelAndProvider("p", "m", "/a");
				});
			},
			true,
			settings,
		);
		const input = new PassThrough();
		vi.spyOn(process, "stdin", "get").mockReturnValue(input as unknown as typeof process.stdin);
		const error = vi.spyOn(console, "error").mockImplementation(() => {});
		let finish!: () => void;
		const exited = new Promise<void>((done) => {
			finish = done;
		});
		const exit = vi.spyOn(process, "exit").mockImplementation(() => {
			finish();
			return undefined as never;
		});
		try {
			void runRpcMode(h.runtime);
			await vi.waitFor(() => {
				expect(input.listenerCount("end")).toBeGreaterThan(0);
			});
			input.end();
			await exited;
			expect(error.mock.calls.flat().join(" ")).toContain("Last-used models");
			expect(output.write.mock.calls.flat().join(" ")).not.toContain("Last-used models");
			expect(output.stderrFlush.mock.invocationCallOrder[0]).toBeLessThan(exit.mock.invocationCallOrder[0]);
			expect(exit).toHaveBeenCalledWith(0);
		} finally {
			input.destroy();
			await h.dispose();
		}
	});
});
