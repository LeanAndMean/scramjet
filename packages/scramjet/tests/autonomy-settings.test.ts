import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	applyRecommendations,
	defaultConfigPath,
	loadAutonomyConfig,
	loadAutonomyConfigResult,
	lookupEdge,
	lookupPublicationPolicy,
	mergeAllRecommendations,
	parseAutonomyConfig,
	parseAutonomyRecommendations,
	resetCache,
	resolveEdgeBehavior,
	resolvePublicationPolicy,
	saveAutonomyConfig,
	updateAutonomyConfig,
	validateConfig,
	validateRecommendations,
} from "../src/autonomy-settings.js";
import type { AutonomyConfig, AutonomyRecommendations, CommandRegistry } from "../src/types.js";

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

	it("rejects unknown top-level keys and non-map roots", () => {
		expect(() => parseAutonomyConfig("foo: bar")).toThrow("unknown top-level key foo");
		expect(() => parseAutonomyConfig("value")).toThrow("root must be a map");
		expect(() => parseAutonomyConfig("[]")).toThrow("root must be a map");
	});

	it("returns empty edges when edges is null", () => {
		expect(parseAutonomyConfig("edges:")).toEqual({ edges: {} });
	});

	it("rejects unknown edge settings and malformed target maps", () => {
		expect(() => parseAutonomyConfig("edges:\n  cmd:a:\n    cmd:b: auto\n")).toThrow("invalid edge setting");
		expect(() => parseAutonomyConfig("edges:\n  cmd:a: not-an-object\n")).toThrow("must be a target map");
		expect(() => parseAutonomyConfig("edges: []\n")).toThrow("edges must be a command map");
	});

	it("parses publication-only user overrides", () => {
		const config = parseAutonomyConfig(`
publications:
  mach12:pr-review:
    add_pr_comment: auto-approve
    create_issue: always-ask
`);
		expect(config).toEqual({
			edges: {},
			publications: {
				"mach12:pr-review": { add_pr_comment: "auto-approve", create_issue: "always-ask" },
			},
		});
	});

	it("rejects invalid publication settings instead of silently disabling approval", () => {
		expect(() => parseAutonomyConfig("publications:\n  mach12:review:\n    add_pr_comment: maybe\n")).toThrow(
			/invalid publication setting/,
		);
	});

	it("throws on malformed YAML", () => {
		expect(() => parseAutonomyConfig("{ invalid yaml: [")).toThrow();
	});
});

describe("lookupPublicationPolicy", () => {
	const defaults = {
		edges: {},
		publications: { "mach12:pr-review": { add_pr_comment: "auto-approve" as const } },
	};

	it("layers exact user overrides over active command defaults", () => {
		expect(lookupPublicationPolicy(null, defaults, "mach12:pr-review", "add_pr_comment")).toBe("auto-approve");
		expect(
			lookupPublicationPolicy(
				{ edges: {}, publications: { "mach12:pr-review": { add_pr_comment: "always-ask" } } },
				defaults,
				"mach12:pr-review",
				"add_pr_comment",
			),
		).toBe("require-approval");
		expect(
			lookupPublicationPolicy(
				{ edges: {}, publications: { "mach12:other": { create_issue: "auto-approve" } } },
				defaults,
				"mach12:other",
				"create_issue",
			),
		).toBe("auto-approve");
	});

	it("preserves unoverridden defaults alongside a sparse exact override", () => {
		const mixedDefaults = {
			edges: {},
			publications: {
				"mach12:review": { create_issue: "require-approval", add_pr_comment: "auto-approve" },
			},
		} satisfies AutonomyRecommendations;
		const config: AutonomyConfig = {
			edges: {},
			publications: { "mach12:review": { create_issue: "auto-approve" } },
		};
		expect(resolvePublicationPolicy(config, mixedDefaults, "mach12:review", "create_issue")).toEqual({
			policy: "auto-approve",
			authorization: "user-override",
		});
		expect(resolvePublicationPolicy(config, mixedDefaults, "mach12:review", "add_pr_comment")).toEqual({
			policy: "auto-approve",
			authorization: "command-default",
		});
	});

	it("requires approval outside an active command or without a default", () => {
		expect(lookupPublicationPolicy(null, defaults, null, "add_pr_comment")).toBe("require-approval");
		expect(lookupPublicationPolicy(null, defaults, "mach12:other", "add_pr_comment")).toBe("require-approval");
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

	it("distinguishes missing, valid, and invalid load states", () => {
		expect(loadAutonomyConfigResult(configPath)).toEqual({ status: "missing" });
		fs.writeFileSync(configPath, "edges: {}\n");
		expect(loadAutonomyConfigResult(configPath, true)).toEqual({ status: "valid", config: { edges: {} } });
		fs.writeFileSync(configPath, "publication: {}\n");
		expect(loadAutonomyConfigResult(configPath, true)).toMatchObject({ status: "invalid" });
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

	it("reloads an atomic replacement even when mtime is preserved", () => {
		fs.writeFileSync(configPath, "edges:\n  cmd:a:\n    cmd:b: chain\n");
		const first = loadAutonomyConfig(configPath);
		const stat = fs.statSync(configPath);
		const replacement = `${configPath}.replacement`;
		fs.writeFileSync(replacement, "edges:\n  cmd:a:\n    cmd:b: pause\n");
		fs.utimesSync(replacement, stat.atime, stat.mtime);
		fs.renameSync(replacement, configPath);
		const second = loadAutonomyConfig(configPath);
		expect(second).not.toBe(first);
		expect(second!.edges["cmd:a"]).toEqual({ "cmd:b": "pause" });
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

	it("keeps repeated reads of malformed YAML fail-closed", () => {
		fs.writeFileSync(configPath, "{ invalid yaml: [");
		expect(() => loadAutonomyConfig(configPath)).toThrow();
		expect(() => loadAutonomyConfig(configPath)).toThrow(/autonomy\.yaml: failed to load config/);
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

	it("does not remove a lock owned by a live process", () => {
		fs.mkdirSync(path.dirname(configPath), { recursive: true });
		fs.writeFileSync(`${configPath}.lock`, JSON.stringify({ pid: process.pid, token: "active-owner" }));
		expect(() => saveAutonomyConfig(configPath, { edges: { "cmd:a": { "cmd:b": "chain" } } })).toThrow(
			"update lock is busy",
		);
		expect(JSON.parse(fs.readFileSync(`${configPath}.lock`, "utf-8"))).toEqual({
			pid: process.pid,
			token: "active-owner",
		});
		expect(fs.existsSync(configPath)).toBe(false);
	});

	it("cleans prepared owner artifacts left by dead processes", () => {
		fs.mkdirSync(path.dirname(configPath), { recursive: true });
		const lockPath = `${configPath}.lock`;
		fs.writeFileSync(`${lockPath}.owner.99999999.dead`, JSON.stringify({ pid: 99999999, token: "dead" }));
		fs.writeFileSync(`${lockPath}.reclaim.owner.99999999.dead`, JSON.stringify({ pid: 99999999, token: "dead" }));
		saveAutonomyConfig(configPath, { edges: { "cmd:a": { "cmd:b": "chain" } } });
		expect(fs.readdirSync(path.dirname(configPath)).some((name) => name.includes(".owner."))).toBe(false);
	});

	it("recovers an orphaned reclaim claim from a dead process", () => {
		fs.mkdirSync(path.dirname(configPath), { recursive: true });
		const lockPath = `${configPath}.lock`;
		const claimToken = "dead-claim";
		fs.writeFileSync(lockPath, "99999999");
		fs.writeFileSync(
			`${lockPath}.reclaim`,
			JSON.stringify({ pid: 99999999, token: claimToken, lockToken: "legacy" }),
		);
		fs.linkSync(lockPath, `${lockPath}.reclaim-snapshot.${claimToken}`);
		saveAutonomyConfig(configPath, { edges: { "cmd:a": { "cmd:b": "chain" } } });
		expect(loadAutonomyConfig(configPath, true)?.edges["cmd:a"]?.["cmd:b"]).toBe("chain");
		expect(fs.readdirSync(path.dirname(configPath)).some((name) => name.includes("reclaim"))).toBe(false);
	});

	it("reclaims a lock whose recorded process no longer exists", () => {
		fs.mkdirSync(path.dirname(configPath), { recursive: true });
		fs.writeFileSync(`${configPath}.lock`, "99999999");
		saveAutonomyConfig(configPath, { edges: { "cmd:a": { "cmd:b": "chain" } } });
		expect(loadAutonomyConfig(configPath, true)?.edges["cmd:a"]?.["cmd:b"]).toBe("chain");
		expect(fs.existsSync(`${configPath}.lock`)).toBe(false);
	});

	it("preserves unrelated sparse changes through locked updates", () => {
		saveAutonomyConfig(configPath, {
			edges: { "cmd:a": { "cmd:b": "chain" } },
			publications: { "cmd:a": { create_issue: "always-ask" } },
		});
		updateAutonomyConfig(configPath, (config) => {
			config.publications ??= {};
			config.publications["cmd:b"] = { add_pr_comment: "auto-approve" };
			return true;
		});
		expect(loadAutonomyConfig(configPath, true)).toEqual({
			edges: { "cmd:a": { "cmd:b": "chain" } },
			publications: {
				"cmd:a": { create_issue: "always-ask" },
				"cmd:b": { add_pr_comment: "auto-approve" },
			},
		});
		expect(fs.readdirSync(tmpDir).some((name) => name.endsWith(".lock") || name.endsWith(".tmp"))).toBe(false);
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

	it("accepts recommendations type (widened parameter)", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"mach12:issue-implement": { "mach12:pr-create": "chain" },
			},
		};
		expect(validateConfig(recs, registry)).toEqual([]);
	});
});

describe("parseAutonomyRecommendations", () => {
	it("parses valid values: chain, pause, default", () => {
		const raw = `
edges:
  cmd:a:
    cmd:b: chain
    cmd:c: pause
    cmd:d: default
`;
		const recs = parseAutonomyRecommendations(raw);
		expect(recs.edges["cmd:a"]).toEqual({
			"cmd:b": "chain",
			"cmd:c": "pause",
			"cmd:d": "default",
		});
	});

	it("skips invalid values", () => {
		const raw = `
edges:
  cmd:a:
    cmd:b: chain
    cmd:c: auto
    cmd:d: 123
`;
		const recs = parseAutonomyRecommendations(raw);
		expect(recs.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
	});

	it("emits warnings for invalid string values when warnings array is provided", () => {
		const raw = `
edges:
  cmd:a:
    cmd:b: chain
    cmd:c: Chain
    cmd:d: chian
`;
		const warnings: string[] = [];
		const recs = parseAutonomyRecommendations(raw, warnings);
		expect(recs.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
		expect(warnings).toHaveLength(2);
		expect(warnings[0]).toContain('"Chain"');
		expect(warnings[0]).toContain("cmd:a");
		expect(warnings[0]).toContain("cmd:c");
		expect(warnings[1]).toContain('"chian"');
		expect(warnings[1]).toContain("cmd:d");
	});

	it("accepts explicit publication defaults and rejects legacy ambiguous values", () => {
		const warnings: string[] = [];
		const recs = parseAutonomyRecommendations(
			"publications:\n  cmd:a:\n    create_issue: require-approval\n    create_pr: auto-approve\n    add_issue_comment: ask\n    add_pr_comment: approve\n",
			warnings,
		);
		expect(recs.publications).toEqual({
			"cmd:a": { create_issue: "require-approval", create_pr: "auto-approve" },
		});
		expect(warnings).toHaveLength(2);
		expect(warnings[0]).toContain("invalid publication setting ask");
		expect(warnings[1]).toContain("invalid publication setting approve");
	});

	it("returns empty edges for empty YAML", () => {
		expect(parseAutonomyRecommendations("")).toEqual({ edges: {} });
	});

	it("returns empty edges for YAML without edges key", () => {
		expect(parseAutonomyRecommendations("foo: bar")).toEqual({ edges: {} });
	});

	it("returns empty edges when edges is null", () => {
		expect(parseAutonomyRecommendations("edges:")).toEqual({ edges: {} });
	});

	it("skips source with non-object targets", () => {
		const raw = `
edges:
  cmd:a: not-an-object
  cmd:b:
    cmd:c: default
`;
		const recs = parseAutonomyRecommendations(raw);
		expect(recs.edges["cmd:a"]).toBeUndefined();
		expect(recs.edges["cmd:b"]).toEqual({ "cmd:c": "default" });
	});

	it("throws on malformed YAML", () => {
		expect(() => parseAutonomyRecommendations("{ invalid yaml: [")).toThrow();
	});

	it("omits source entries with no valid targets", () => {
		const raw = `
edges:
  cmd:a:
    cmd:b: invalid
  cmd:c:
    cmd:d: chain
`;
		const recs = parseAutonomyRecommendations(raw);
		expect(recs.edges["cmd:a"]).toBeUndefined();
		expect(recs.edges["cmd:c"]).toEqual({ "cmd:d": "chain" });
	});
});

describe("applyRecommendations", () => {
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

	it("fills gaps: absent edge gets added", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "chain" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 1, skipped: 0 });
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
	});

	it("does not overwrite existing edge", () => {
		const existing: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "pause" } },
		};
		saveAutonomyConfig(configPath, existing);

		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "chain" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 0, skipped: 1 });
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "pause" });
	});

	it("default recommendation is a no-op", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "default" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 0, skipped: 1 });
		expect(fs.existsSync(configPath)).toBe(false);
	});

	it("default on existing edge is skipped (preserves user setting)", () => {
		const existing: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "chain" } },
		};
		saveAutonomyConfig(configPath, existing);

		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "default" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 0, skipped: 1 });
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
	});

	it("returns correct counts for mixed apply/skip", () => {
		const existing: AutonomyConfig = {
			edges: { "cmd:a": { "cmd:b": "pause" } },
		};
		saveAutonomyConfig(configPath, existing);

		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "chain", "cmd:c": "chain" },
				"cmd:d": { "cmd:e": "pause", "cmd:f": "default" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		// cmd:a→cmd:b: skipped (existing), cmd:a→cmd:c: applied,
		// cmd:d→cmd:e: applied, cmd:d→cmd:f: skipped (default)
		expect(result).toEqual({ applied: 2, skipped: 2 });
		const loaded = loadAutonomyConfig(configPath);
		expect(loaded!.edges["cmd:a"]).toEqual({ "cmd:b": "pause", "cmd:c": "chain" });
		expect(loaded!.edges["cmd:d"]).toEqual({ "cmd:e": "pause" });
	});

	it("all-default recommendations leave file absent", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"cmd:a": { "cmd:b": "default", "cmd:c": "default" },
			},
		};
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 0, skipped: 2 });
		expect(fs.existsSync(configPath)).toBe(false);
	});

	it("empty recommendations is a no-op", () => {
		const recs: AutonomyRecommendations = { edges: {} };
		const result = applyRecommendations(configPath, recs);
		expect(result).toEqual({ applied: 0, skipped: 0 });
	});
});

describe("mergeAllRecommendations", () => {
	it("merges multiple sets", () => {
		const recs = new Map<string, AutonomyRecommendations>([
			["set-a", { edges: { "cmd:a": { "cmd:b": "chain" } } }],
			["set-b", { edges: { "cmd:c": { "cmd:d": "pause" } } }],
		]);
		const merged = mergeAllRecommendations(recs);
		expect(merged.edges["cmd:a"]).toEqual({ "cmd:b": "chain" });
		expect(merged.edges["cmd:c"]).toEqual({ "cmd:d": "pause" });
	});

	it("first-write-wins on conflicts", () => {
		const recs = new Map<string, AutonomyRecommendations>([
			["set-a", { edges: { "cmd:a": { "cmd:b": "chain" } } }],
			["set-b", { edges: { "cmd:a": { "cmd:b": "pause" } } }],
		]);
		const merged = mergeAllRecommendations(recs);
		expect(merged.edges["cmd:a"]!["cmd:b"]).toBe("chain");
	});

	it("empty map produces empty edges", () => {
		const merged = mergeAllRecommendations(new Map());
		expect(merged.edges).toEqual({});
	});

	it("merges different targets for same source across sets", () => {
		const recs = new Map<string, AutonomyRecommendations>([
			["set-a", { edges: { "cmd:a": { "cmd:b": "chain" } } }],
			["set-b", { edges: { "cmd:a": { "cmd:c": "pause" } } }],
		]);
		const merged = mergeAllRecommendations(recs);
		expect(merged.edges["cmd:a"]).toEqual({ "cmd:b": "chain", "cmd:c": "pause" });
	});
});

describe("validateRecommendations", () => {
	const registry: CommandRegistry = new Map([
		["mach12:issue-implement", {} as any],
		["mach12:pr-create", {} as any],
	]);

	it("returns no warnings for valid recommendations", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"mach12:issue-implement": { "mach12:pr-create": "chain" },
			},
		};
		expect(validateRecommendations(recs, registry)).toEqual([]);
	});

	it("warns on unknown commands", () => {
		const recs: AutonomyRecommendations = {
			edges: {
				"mach12:unknown": { "mach12:pr-create": "chain" },
			},
		};
		const warnings = validateRecommendations(recs, registry);
		expect(warnings).toContain('unknown source command "mach12:unknown"');
	});
});
