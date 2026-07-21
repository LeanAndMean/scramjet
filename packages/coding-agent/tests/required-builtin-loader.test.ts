import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { DefaultResourceLoaderOptions } from "../src/core/resource-loader.js";
import { DefaultResourceLoader, describeRuntimeError, RequiredBuiltinInitError } from "../src/core/resource-loader.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import type { SourceInfo } from "../src/core/source-info.js";

function makeLoader(options: Partial<Omit<DefaultResourceLoaderOptions, "cwd" | "agentDir">> = {}): {
	loader: DefaultResourceLoader;
	settingsManager: SettingsManager;
	dir: string;
} {
	const dir = mkdtempSync(join(tmpdir(), "required-builtin-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");
	const settingsManager = options.settingsManager ?? SettingsManager.inMemory();
	const loader = new DefaultResourceLoader({ ...options, cwd, agentDir, settingsManager });
	return { loader, settingsManager, dir };
}

/** A builtin that succeeds on its first invocation and throws on every later one. */
function succeedThenThrow(error: Error): () => void {
	let calls = 0;
	return () => {
		calls += 1;
		if (calls > 1) {
			throw error;
		}
	};
}

describe("required builtin init contract", () => {
	it("throws a product-attributed error preserving the cause when the builtin throws synchronously", async () => {
		const original = new Error("kaboom");
		const { loader } = makeLoader({
			builtinInit: () => {
				throw original;
			},
		});

		let caught: unknown;
		try {
			await loader.reload();
		} catch (error) {
			caught = error;
		}

		expect(caught).toBeInstanceOf(RequiredBuiltinInitError);
		expect((caught as Error).message).toMatch(/scramjet/i);
		expect((caught as Error).cause).toBe(original);
		expect((caught as Error).stack).toBeTruthy();
	});

	it("throws a product-attributed error preserving the cause when the builtin rejects asynchronously", async () => {
		const original = new Error("async kaboom");
		const { loader } = makeLoader({
			builtinInit: async () => {
				throw original;
			},
		});

		let caught: unknown;
		try {
			await loader.reload();
		} catch (error) {
			caught = error;
		}

		expect(caught).toBeInstanceOf(RequiredBuiltinInitError);
		expect((caught as Error).cause).toBe(original);
	});

	it("loads a successful builtin and reflects it in the extension result", async () => {
		const { loader } = makeLoader({ builtinInit: () => {} });
		await loader.reload();
		const extensions = loader.getExtensions().extensions;
		expect(extensions.some((e) => e.path === "<builtin>")).toBe(true);
		expect(loader.getExtensions().errors.some((e) => e.path === "<builtin>")).toBe(false);
	});
});

describe("optional extension failures stay in the diagnostic channel", () => {
	it("does not throw when an optional disk extension path is missing", async () => {
		const missing = join(mkdtempSync(join(tmpdir(), "required-builtin-missing-")), "missing-extension.ts");
		const { loader } = makeLoader({
			additionalExtensionPaths: [missing],
		});

		await expect(loader.reload()).resolves.toBeUndefined();
		const errors = loader.getExtensions().errors;
		expect(errors.some((e) => e.path.includes("missing-extension"))).toBe(true);
	});

	it("does not throw when an inline extension factory throws", async () => {
		const { loader } = makeLoader({
			extensionFactories: [
				() => {
					throw new Error("inline boom");
				},
			],
		});

		await expect(loader.reload()).resolves.toBeUndefined();
		const errors = loader.getExtensions().errors;
		expect(errors.some((e) => e.path === "<inline:1>" && e.error.includes("inline boom"))).toBe(true);
	});
});

describe("failed builtin init leaves previously committed loader state intact", () => {
	it("preserves the assembled extension result", async () => {
		const { loader } = makeLoader({ builtinInit: succeedThenThrow(new Error("second load")) });
		await loader.reload();
		const before = loader.getExtensions();

		await expect(loader.reload()).rejects.toBeInstanceOf(RequiredBuiltinInitError);

		expect(loader.getExtensions()).toBe(before);
	});

	// Scoped to the standalone loader path (a bare `loader.reload()`, as used by createAgentSessionServices and
	// sdk.ts): the loader itself performs no settings reload until its post-builtin commit block, so a failing
	// load never reloads settings. This does NOT hold for AgentSession.reload(), which reloads settings before
	// the builtin can throw (see agent-session.ts).
	it("standalone loader preserves settings (no settings reload on the failing load)", async () => {
		const settingsManager = SettingsManager.inMemory();
		const { loader } = makeLoader({ settingsManager, builtinInit: succeedThenThrow(new Error("second load")) });
		await loader.reload();

		settingsManager.applyOverrides({ theme: "marker-theme" });
		expect(settingsManager.getTheme()).toBe("marker-theme");

		await expect(loader.reload()).rejects.toBeInstanceOf(RequiredBuiltinInitError);

		expect(settingsManager.getTheme()).toBe("marker-theme");
	});

	it("preserves extension source-info maps (no clearing on the failing load)", async () => {
		const { loader } = makeLoader({ builtinInit: succeedThenThrow(new Error("second load")) });
		await loader.reload();

		const marker: SourceInfo = {
			path: "marker-path",
			source: "test",
			scope: "temporary",
			origin: "top-level",
		};
		const maps = loader as unknown as {
			extensionSkillSourceInfos: Map<string, SourceInfo>;
			extensionPromptSourceInfos: Map<string, SourceInfo>;
			extensionThemeSourceInfos: Map<string, SourceInfo>;
		};
		maps.extensionSkillSourceInfos.set("marker-path", marker);
		maps.extensionPromptSourceInfos.set("marker-path", marker);
		maps.extensionThemeSourceInfos.set("marker-path", marker);

		await expect(loader.reload()).rejects.toBeInstanceOf(RequiredBuiltinInitError);

		expect(maps.extensionSkillSourceInfos.has("marker-path")).toBe(true);
		expect(maps.extensionPromptSourceInfos.has("marker-path")).toBe(true);
		expect(maps.extensionThemeSourceInfos.has("marker-path")).toBe(true);
	});
});

describe("describeRuntimeError", () => {
	it("appends the unwrapped cause chain for a RequiredBuiltinInitError", () => {
		const root = new Error("root boom");
		const wrapper = new Error("wrapper", { cause: root });
		const text = describeRuntimeError(new RequiredBuiltinInitError(wrapper));
		expect(text.startsWith("Scramjet product initialization failed\n")).toBe(true);
		expect(text).toContain("wrapper");
		expect(text).toContain("Caused by: ");
		expect(text).toContain("root boom");
	});

	it("returns only the product message when there is no cause", () => {
		expect(describeRuntimeError(new RequiredBuiltinInitError(undefined))).toBe(
			"Scramjet product initialization failed",
		);
	});

	it("passes other errors and non-errors through unchanged", () => {
		expect(describeRuntimeError(new Error("plain"))).toBe("plain");
		expect(describeRuntimeError("stringy")).toBe("stringy");
	});
});
