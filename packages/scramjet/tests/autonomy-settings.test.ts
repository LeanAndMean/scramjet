import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	defaultConfigPath,
	loadAutonomyConfig,
	lookupEdge,
	parseAutonomyConfig,
	resetCache,
	resolveEdgeBehavior,
	saveAutonomyConfig,
	validateConfig,
} from "../src/autonomy-settings.js";
import type { AutonomyConfig, CommandRegistry } from "../src/types.js";

describe("parseAutonomyConfig", () => {
	it("parses valid config", () => {
		const raw = `
edges:
  mach12:issue-implement:
    mach12:issue-implement: chain
    mach12:pr-create: pause
    "*": pause
  mach12:pr-pre-merge:
    mach12:pr-merge: chain
`;
		const config = parseAutonomyConfig(raw);
		expect(config.edges["mach12:issue-implement"]).toEqual({
			"mach12:issue-implement": "chain",
			"mach12:pr-create": "pause",
			"*": "pause",
		});
		expect(config.edges["mach12:pr-pre-merge"]).toEqual({
			"mach12:pr-merge": "chain",
		});
	});

	it("returns empty edges for empty YAML", () => {
		expect(parseAutonomyConfig("")).toEqual({ edges: {} });
	});

	it("returns empty edges for YAML without edges key", () => {
		expect(parseAutonomyConfig("foo: bar")).toEqual({ edges: {} });
	});

	it("returns empty edges when edges is null", () => {
		expect(parseAutonomyConfig("edges:")).toEqual({ edges: {} });
	});

	it("skips unknown setting values", () => {
		const raw = `
edges:
  cmd:a:
    cmd:b: chain
    cmd:c: auto
    cmd:d: pause
    cmd:e: 123
`;
		const config = parseAutonomyConfig(raw);
		expect(config.edges["cmd:a"]).toEqual({
			"cmd:b": "chain",
			"cmd:d": "pause",
		});
	});

	it("skips source with non-object targets", () => {
		const raw = `
edges:
  cmd:a: not-an-object
  cmd:b:
    cmd:c: chain
`;
		const config = parseAutonomyConfig(raw);
		expect(config.edges["cmd:a"]).toBeUndefined();
		expect(config.edges["cmd:b"]).toEqual({ "cmd:c": "chain" });
	});

	it("throws on malformed YAML", () => {
		expect(() => parseAutonomyConfig("{ invalid yaml: [")).toThrow();
	});
});

describe("lookupEdge", () => {
	const config: AutonomyConfig = {
		edges: {
			"mach12:issue-implement": {
				"mach12:issue-implement": "chain",
				"mach12:pr-create": "pause",
				"*": "pause",
			},
			"mach12:pr-pre-merge": {
				"mach12:pr-merge": "chain",
			},
		},
	};

	it("returns exact match", () => {
		expect(lookupEdge(config, "mach12:issue-implement", "mach12:issue-implement")).toBe("chain");
		expect(lookupEdge(config, "mach12:issue-implement", "mach12:pr-create")).toBe("pause");
		expect(lookupEdge(config, "mach12:pr-pre-merge", "mach12:pr-merge")).toBe("chain");
	});

	it("falls back to wildcard", () => {
		expect(lookupEdge(config, "mach12:issue-implement", "mach12:unknown")).toBe("pause");
	});

	it("returns null for absent source", () => {
		expect(lookupEdge(config, "mach12:nonexistent", "mach12:anything")).toBeNull();
	});

	it("returns null for absent target without wildcard", () => {
		expect(lookupEdge(config, "mach12:pr-pre-merge", "mach12:unknown")).toBeNull();
	});

	it("returns null for null config", () => {
		expect(lookupEdge(null, "mach12:issue-implement", "mach12:pr-create")).toBeNull();
	});
});

describe("loadAutonomyConfig", () => {
	let tmpDir: string;
	let configPath: string;

	beforeEach(() => {
		resetCache();
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-test-"));
		configPath = path.join(tmpDir, "autonomy.yaml");
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns null for missing file", () => {
		expect(loadAutonomyConfig(configPath)).toBeNull();
	});

	it("loads valid config from file", () => {
		fs.writeFileSync(
			configPath,
			`
edges:
  cmd:a:
    cmd:b: chain
`,
		);
		const config = loadAutonomyConfig(configPath);
		expect(config).not.toBeNull();
		expect(config!.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
	});

	it("caches by mtime", () => {
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: chain\n");
		const first = loadAutonomyConfig(configPath);
		const second = loadAutonomyConfig(configPath);
		expect(first).toBe(second);
	});

	it("reloads when mtime changes", () => {
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: chain\n");
		const first = loadAutonomyConfig(configPath);

		// Advance mtime by touching the file with new content
		const futureTime = Date.now() + 2000;
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: pause\n");
		fs.utimesSync(configPath, futureTime / 1000, futureTime / 1000);

		const second = loadAutonomyConfig(configPath);
		expect(second).not.toBe(first);
		expect(second!.edges["cmd:a"]).toEqual({ "cmd:b": "pause" });
	});

	it("returns null after file is deleted", () => {
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: chain\n");
		loadAutonomyConfig(configPath);
		fs.unlinkSync(configPath);
		expect(loadAutonomyConfig(configPath)).toBeNull();
	});

	it("throws on malformed YAML with a descriptive message", () => {
		fs.writeFileSync(configPath, "{ invalid yaml: [");
		expect(() => loadAutonomyConfig(configPath)).toThrow(/autonomy\.yaml: failed to load config/);
	});

	it("caches null after malformed YAML so repeated calls return null", () => {
		fs.writeFileSync(configPath, "{ invalid yaml: [");
		expect(() => loadAutonomyConfig(configPath)).toThrow();
		// Second call with same mtime returns cached null without re-throwing
		expect(loadAutonomyConfig(configPath)).toBeNull();
	});

	it("throws on permission errors from stat with a descriptive message", () => {
		// Use a path we know will fail with something other than ENOENT
		const badPath = path.join(configPath, "nested", "impossible");
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: chain\n");
		// Trying to stat a path through a file (not a dir) gives ENOTDIR
		expect(() => loadAutonomyConfig(badPath)).toThrow(/autonomy\.yaml: cannot stat config file/);
	});
});

describe("resolveEdgeBehavior", () => {
	let tmpDir: string;
	let configPath: string;

	beforeEach(() => {
		resetCache();
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-test-"));
		configPath = path.join(tmpDir, "autonomy.yaml");
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns null when no config file exists", () => {
		expect(resolveEdgeBehavior(configPath, "cmd:a", "cmd:b")).toBeNull();
	});

	it("returns setting from file", () => {
		fs.writeFileSync(
			configPath,
			`
edges:
  cmd:a:
    cmd:b: chain
    cmd:c: pause
`,
		);
		expect(resolveEdgeBehavior(configPath, "cmd:a", "cmd:b")).toBe("chain");
		expect(resolveEdgeBehavior(configPath, "cmd:a", "cmd:c")).toBe("pause");
		expect(resolveEdgeBehavior(configPath, "cmd:a", "cmd:d")).toBeNull();
	});
});

describe("defaultConfigPath", () => {
	it("uses XDG_CONFIG_HOME when set", () => {
		const original = process.env.XDG_CONFIG_HOME;
		try {
			process.env.XDG_CONFIG_HOME = "/custom/config";
			expect(defaultConfigPath()).toBe("/custom/config/scramjet/autonomy.yaml");
		} finally {
			if (original === undefined) delete process.env.XDG_CONFIG_HOME;
			else process.env.XDG_CONFIG_HOME = original;
		}
	});

	it("falls back to ~/.config", () => {
		const original = process.env.XDG_CONFIG_HOME;
		try {
			delete process.env.XDG_CONFIG_HOME;
			expect(defaultConfigPath()).toBe(path.join(os.homedir(), ".config", "scramjet", "autonomy.yaml"));
		} finally {
			if (original !== undefined) process.env.XDG_CONFIG_HOME = original;
		}
	});
});

describe("saveAutonomyConfig", () => {
	let tmpDir: string;
	let configPath: string;

	beforeEach(() => {
		resetCache();
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-test-"));
		configPath = path.join(tmpDir, "scramjet", "autonomy.yaml");
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("creates parent directories and writes YAML", () => {
		const config: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "chain" } },
		};
		saveAutonomyConfig(configPath, config);
		expect(fs.existsSync(configPath)).toBe(true);
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
	});

	it("roundtrips config through save and load", () => {
		const config: AutonomyConfig = {
			edges: {
				"cmd:a": { "cmd:b": "chain", "cmd:c": "pause" },
				"cmd:d": { "*": "pause" },
			},
		};
		saveAutonomyConfig(configPath, config);
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded).toEqual(config);
	});

	it("removes source entries with no valid targets", () => {
		const config: AutonomyConfig = {
			edges: {
				"cmd:a": {} as any,
				"cmd:b": { "cmd:c": "chain" },
			},
		};
		saveAutonomyConfig(configPath, config);
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toBeUndefined();
		expect(loaded!.edges["cmd:b"]).toEqual({ "cmd:c": "chain" });
	});

	it("deletes config file when all edges are empty", () => {
		const config: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "chain" } },
		};
		saveAutonomyConfig(configPath, config);
		expect(fs.existsSync(configPath)).toBe(true);

		saveAutonomyConfig(configPath, { edges: {} });
		expect(fs.existsSync(configPath)).toBe(false);
	});

	it("handles delete when file does not exist", () => {
		expect(() => saveAutonomyConfig(configPath, { edges: {} })).not.toThrow();
	});

	it("resets cache after save", () => {
		const config1: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "chain" } },
		};
		saveAutonomyConfig(configPath, config1);
		loadAutonomyConfig(configPath);

		const config2: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "pause" } },
		};
		saveAutonomyConfig(configPath, config2);
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "pause" });
	});

	it("writes atomically (no .tmp file left behind)", () => {
		const config: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "chain" } },
		};
		saveAutonomyConfig(configPath, config);
		const dir = path.dirname(configPath);
		const files = fs.readdirSync(dir);
		expect(files).not.toContain("autonomy.yaml.tmp");
		expect(files).toContain("autonomy.yaml");
	});
});

describe("validateConfig", () => {
	const registry: CommandRegistry = new Map([
		["mach12:issue-implement", {} as any],
		["mach12:pr-create", {} as any],
		["mach12:pr-merge", {} as any],
	]);

	it("returns no warnings for valid config", () => {
		const config: AutonomyConfig = {
			edges: {
				"mach12:issue-implement": {
					"mach12:issue-implement": "chain",
					"mach12:pr-create": "pause",
				},
			},
		};
		expect(validateConfig(config, registry)).toEqual([]);
	});

	it("warns on unknown source command", () => {
		const config: AutonomyConfig = {
			edges: {
				"mach12:nonexistent": { "mach12:pr-create": "chain" },
			},
		};
		const warnings = validateConfig(config, registry);
		expect(warnings).toContain('unknown source command "mach12:nonexistent"');
	});

	it("warns on unknown target command", () => {
		const config: AutonomyConfig = {
			edges: {
				"mach12:issue-implement": { "mach12:unknown": "chain" },
			},
		};
		const warnings = validateConfig(config, registry);
		expect(warnings).toContain('unknown target command "mach12:unknown" (in mach12:issue-implement)');
	});

	it("does not warn on wildcard target", () => {
		const config: AutonomyConfig = {
			edges: {
				"mach12:issue-implement": { "*": "pause" },
			},
		};
		expect(validateConfig(config, registry)).toEqual([]);
	});

	it("reports multiple warnings", () => {
		const config: AutonomyConfig = {
			edges: {
				"bad:source": { "bad:target": "chain", "mach12:pr-merge": "pause" },
			},
		};
		const warnings = validateConfig(config, registry);
		expect(warnings).toHaveLength(2);
		expect(warnings[0]).toContain('unknown source command "bad:source"');
		expect(warnings[1]).toContain('unknown target command "bad:target"');
	});
});
