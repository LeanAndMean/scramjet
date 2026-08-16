import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import type {
	AutonomyConfig,
	AutonomyRecommendations,
	CommandRegistry,
	EdgeSetting,
	EffectivePublicationPolicy,
	PublicationDefault,
	PublicationOverride,
	PublicationTool,
	RecommendationSetting,
} from "./types.js";
import { PUBLICATION_TOOLS } from "./types.js";

const VALID_SETTINGS = new Set(["chain", "pause"]);
const VALID_REC_SETTINGS = new Set(["chain", "pause", "default"]);
const VALID_PUBLICATION_OVERRIDES = new Set(["always-ask", "auto-approve"]);
const VALID_PUBLICATION_DEFAULTS = new Set(["require-approval", "auto-approve"]);

let cache: {
	path: string;
	mtimeMs: number;
	ctimeMs: number;
	size: number;
	ino: number;
	config?: AutonomyConfig;
	error?: Error;
} | null = null;

export type AutonomyConfigLoadResult =
	| { status: "missing" }
	| { status: "valid"; config: AutonomyConfig }
	| { status: "invalid"; error: Error };

const LOCK_RETRY_MS = 10;
const LOCK_ATTEMPTS = 50;
const INCOMPLETE_LOCK_STALE_MS = 5_000;

export function defaultConfigPath(): string {
	const configHome = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config");
	return path.join(configHome, "scramjet", "autonomy.yaml");
}

export function parseAutonomyConfig(raw: string): AutonomyConfig {
	const doc = parseYaml(raw);
	if (doc == null) return { edges: {} };
	if (typeof doc !== "object" || Array.isArray(doc)) throw new Error("autonomy.yaml: root must be a map");
	for (const key of Object.keys(doc)) {
		if (key !== "edges" && key !== "publications") throw new Error(`autonomy.yaml: unknown top-level key ${key}`);
	}

	const edgeSource = "edges" in doc ? doc.edges : undefined;
	if (edgeSource != null && (typeof edgeSource !== "object" || Array.isArray(edgeSource)))
		throw new Error("autonomy.yaml: edges must be a command map");
	const edges: AutonomyConfig["edges"] = {};
	for (const [source, targets] of Object.entries((edgeSource ?? {}) as Record<string, unknown>)) {
		if (targets == null || typeof targets !== "object" || Array.isArray(targets))
			throw new Error(`autonomy.yaml: edge settings for ${source} must be a target map`);
		const targetMap: Record<string, "chain" | "pause"> = {};
		for (const [target, setting] of Object.entries(targets as Record<string, unknown>)) {
			if (typeof setting !== "string" || !VALID_SETTINGS.has(setting))
				throw new Error(`autonomy.yaml: invalid edge setting ${String(setting)} for ${source} → ${target}`);
			targetMap[target] = setting as "chain" | "pause";
		}
		if (Object.keys(targetMap).length > 0) edges[source] = targetMap;
	}
	const publications = parsePublicationMap<PublicationOverride>(
		"publications" in doc ? doc.publications : undefined,
		VALID_PUBLICATION_OVERRIDES,
		(message) => {
			throw new Error(`autonomy.yaml: ${message}`);
		},
	);
	return Object.keys(publications).length > 0 ? { edges, publications } : { edges };
}

function parsePublicationMap<T extends string>(
	value: unknown,
	valid: ReadonlySet<string>,
	onInvalid?: (message: string) => void,
): Record<string, Partial<Record<PublicationTool, T>>> {
	const publications: Record<string, Partial<Record<PublicationTool, T>>> = {};
	if (value == null) return publications;
	if (typeof value !== "object" || Array.isArray(value)) {
		onInvalid?.("publications must be a command map");
		return publications;
	}
	for (const [command, settings] of Object.entries(value as Record<string, unknown>)) {
		if (settings == null || typeof settings !== "object" || Array.isArray(settings)) {
			onInvalid?.(`publication settings for ${command} must be a tool map`);
			continue;
		}
		const commandSettings: Partial<Record<PublicationTool, T>> = {};
		for (const [tool, setting] of Object.entries(settings as Record<string, unknown>)) {
			if (!PUBLICATION_TOOLS.includes(tool as PublicationTool)) {
				onInvalid?.(`unknown publication tool ${tool} for ${command}`);
				continue;
			}
			if (typeof setting !== "string" || !valid.has(setting)) {
				onInvalid?.(`invalid publication setting ${String(setting)} for ${command} → ${tool}`);
				continue;
			}
			commandSettings[tool as PublicationTool] = setting as T;
		}
		if (Object.keys(commandSettings).length > 0) publications[command] = commandSettings;
	}
	return publications;
}

export function loadAutonomyConfigResult(configPath: string, fresh = false): AutonomyConfigLoadResult {
	let stat: fs.Stats;
	try {
		stat = fs.statSync(configPath);
	} catch (err: unknown) {
		if (cache?.path === configPath) cache = null;
		if ((err as NodeJS.ErrnoException).code === "ENOENT") return { status: "missing" };
		const msg = err instanceof Error ? err.message : String(err);
		return { status: "invalid", error: new Error(`autonomy.yaml: cannot stat config file: ${msg}`) };
	}

	if (
		!fresh &&
		cache?.path === configPath &&
		stat.mtimeMs === cache.mtimeMs &&
		stat.ctimeMs === cache.ctimeMs &&
		stat.size === cache.size &&
		stat.ino === cache.ino
	) {
		if (cache.error) return { status: "invalid", error: cache.error };
		return { status: "valid", config: cache.config ?? { edges: {} } };
	}

	try {
		const raw = fs.readFileSync(configPath, "utf-8");
		const config = parseAutonomyConfig(raw);
		cache = {
			path: configPath,
			mtimeMs: stat.mtimeMs,
			ctimeMs: stat.ctimeMs,
			size: stat.size,
			ino: stat.ino,
			config,
		};
		return { status: "valid", config };
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		const error = new Error(`autonomy.yaml: failed to load config: ${msg}`);
		cache = {
			path: configPath,
			mtimeMs: stat.mtimeMs,
			ctimeMs: stat.ctimeMs,
			size: stat.size,
			ino: stat.ino,
			error,
		};
		return { status: "invalid", error };
	}
}

export function loadAutonomyConfig(configPath: string, fresh = false): AutonomyConfig | null {
	const result = loadAutonomyConfigResult(configPath, fresh);
	if (result.status === "missing") return null;
	if (result.status === "invalid") throw result.error;
	return result.config;
}

export function lookupEdge(config: AutonomyConfig | null, source: string, target: string): EdgeSetting {
	if (!config) return null;
	const targets = config.edges[source];
	if (!targets) return null;
	return targets[target] ?? targets["*"] ?? null;
}

export function resolveEdgeBehavior(configPath: string, source: string, target: string): EdgeSetting {
	const config = loadAutonomyConfig(configPath);
	return lookupEdge(config, source, target);
}

export interface ResolvedPublicationPolicy {
	policy: EffectivePublicationPolicy;
	authorization: "interactive" | "command-default" | "user-override";
}

export function resolvePublicationPolicy(
	config: AutonomyConfig | null,
	defaults: AutonomyRecommendations,
	command: string | null,
	tool: PublicationTool,
): ResolvedPublicationPolicy {
	if (!command) return { policy: "require-approval", authorization: "interactive" };
	const override = config?.publications?.[command]?.[tool];
	if (override === "always-ask") return { policy: "require-approval", authorization: "interactive" };
	if (override === "auto-approve") return { policy: "auto-approve", authorization: "user-override" };
	const policy = defaults.publications?.[command]?.[tool] ?? "require-approval";
	return { policy, authorization: policy === "auto-approve" ? "command-default" : "interactive" };
}

export function lookupPublicationPolicy(
	config: AutonomyConfig | null,
	defaults: AutonomyRecommendations,
	command: string | null,
	tool: PublicationTool,
): EffectivePublicationPolicy {
	return resolvePublicationPolicy(config, defaults, command, tool).policy;
}

export function validateConfig(
	config: {
		edges: Record<string, Record<string, string>>;
		publications?: Record<string, Record<string, string | undefined>>;
	},
	registry: CommandRegistry,
): string[] {
	const warnings: string[] = [];
	for (const [source, targets] of Object.entries(config.edges)) {
		if (!registry.has(source)) {
			warnings.push(`unknown source command "${source}"`);
		}
		for (const target of Object.keys(targets)) {
			if (target !== "*" && !registry.has(target)) {
				warnings.push(`unknown target command "${target}" (in ${source})`);
			}
		}
	}
	for (const command of Object.keys(config.publications ?? {})) {
		if (!registry.has(command)) warnings.push(`unknown publication command "${command}"`);
	}
	return warnings;
}

export function validateRecommendations(recs: AutonomyRecommendations, registry: CommandRegistry): string[] {
	return validateConfig(recs, registry);
}

export function parseAutonomyRecommendations(raw: string, warnings?: string[]): AutonomyRecommendations {
	const doc = parseYaml(raw);
	if (doc == null || typeof doc !== "object" || Array.isArray(doc)) return { edges: {} };

	const edgeSource = "edges" in doc && doc.edges != null && typeof doc.edges === "object" ? doc.edges : {};
	const edges: AutonomyRecommendations["edges"] = {};
	for (const [source, targets] of Object.entries(edgeSource as Record<string, unknown>)) {
		if (targets == null || typeof targets !== "object") continue;
		const targetMap: Record<string, RecommendationSetting> = {};
		for (const [target, setting] of Object.entries(targets as Record<string, unknown>)) {
			if (typeof setting === "string" && VALID_REC_SETTINGS.has(setting)) {
				targetMap[target] = setting as RecommendationSetting;
			} else if (typeof setting === "string") {
				warnings?.push(
					`[scramjet/discovery] unknown autonomy recommendation value "${setting}" for edge ${source} → ${target} (expected: chain, pause, default)`,
				);
			}
		}
		if (Object.keys(targetMap).length > 0) {
			edges[source] = targetMap;
		}
	}
	const publications = parsePublicationMap<PublicationDefault>(
		"publications" in doc ? doc.publications : undefined,
		VALID_PUBLICATION_DEFAULTS,
		(message) => warnings?.push(`[scramjet/discovery] ${message}`),
	);
	return Object.keys(publications).length > 0 ? { edges, publications } : { edges };
}

export function applyRecommendations(
	configPath: string,
	recommendations: AutonomyRecommendations,
): { applied: number; skipped: number } {
	let applied = 0;
	let skipped = 0;
	updateAutonomyConfig(configPath, (config) => {
		for (const [source, targets] of Object.entries(recommendations.edges)) {
			for (const [target, setting] of Object.entries(targets)) {
				if (setting === "default" || config.edges[source]?.[target]) {
					skipped++;
					continue;
				}
				config.edges[source] ??= {};
				config.edges[source][target] = setting;
				applied++;
			}
		}
		return applied > 0;
	});
	return { applied, skipped };
}

export function mergeAllRecommendations(recs: ReadonlyMap<string, AutonomyRecommendations>): AutonomyRecommendations {
	const merged: AutonomyRecommendations = { edges: {}, publications: {} };
	for (const setRecs of recs.values()) {
		for (const [source, targets] of Object.entries(setRecs.edges)) {
			if (!merged.edges[source]) {
				merged.edges[source] = {};
			}
			for (const [target, setting] of Object.entries(targets)) {
				if (!(target in merged.edges[source])) {
					merged.edges[source][target] = setting;
				}
			}
		}
		for (const [command, settings] of Object.entries(setRecs.publications ?? {})) {
			if (!merged.publications![command]) merged.publications![command] = {};
			for (const [tool, setting] of Object.entries(settings) as [PublicationTool, PublicationDefault][]) {
				if (!(tool in merged.publications![command])) merged.publications![command][tool] = setting;
			}
		}
	}
	if (Object.keys(merged.publications!).length === 0) delete merged.publications;
	return merged;
}

export function saveAutonomyConfig(configPath: string, config: AutonomyConfig): void {
	withConfigLock(configPath, () => writeAutonomyConfigUnlocked(configPath, config));
}

export function updateAutonomyConfig(
	configPath: string,
	update: (config: AutonomyConfig) => boolean,
): { config: AutonomyConfig; changed: boolean } {
	return withConfigLock(configPath, () => {
		const config = loadAutonomyConfig(configPath, true) ?? { edges: {} };
		const changed = update(config);
		if (changed) writeAutonomyConfigUnlocked(configPath, config);
		return { config, changed };
	});
}

function withConfigLock<T>(configPath: string, action: () => T): T {
	const dir = path.dirname(configPath);
	fs.mkdirSync(dir, { recursive: true });
	const lockPath = `${configPath}.lock`;
	cleanupOrphanedPreparedFiles(lockPath);
	let owner: { inode: number; content: string } | undefined;
	for (let attempt = 0; attempt < LOCK_ATTEMPTS; attempt++) {
		const token = randomUUID();
		const content = JSON.stringify({ pid: process.pid, token });
		const preparedPath = `${lockPath}.owner.${process.pid}.${token}`;
		fs.writeFileSync(preparedPath, content, { encoding: "utf-8", flag: "wx", mode: 0o600 });
		let linkError: unknown;
		try {
			fs.linkSync(preparedPath, lockPath);
			owner = { inode: fs.lstatSync(lockPath).ino, content };
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") linkError = error;
		}
		let preparedCleanupError: unknown;
		try {
			fs.unlinkSync(preparedPath);
		} catch (error) {
			preparedCleanupError = error;
		}
		if (linkError !== undefined || preparedCleanupError !== undefined) {
			let rollbackError: unknown;
			if (owner) {
				try {
					if (fs.lstatSync(lockPath).ino !== owner.inode || fs.readFileSync(lockPath, "utf-8") !== owner.content)
						throw new Error("update lock ownership changed during setup rollback");
					fs.unlinkSync(lockPath);
				} catch (error) {
					rollbackError = error;
				}
			}
			throw new AggregateError(
				[linkError, preparedCleanupError, rollbackError].filter((error) => error !== undefined),
				"autonomy update lock setup failed",
			);
		}
		if (owner) break;
		if (reclaimStaleLock(lockPath)) continue;
		Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, LOCK_RETRY_MS);
	}
	if (!owner) throw new Error(`autonomy.yaml: update lock is busy: ${lockPath}`);
	let value: T | undefined;
	let actionError: unknown;
	try {
		value = action();
	} catch (error) {
		actionError = error;
	}
	let cleanupError: unknown;
	try {
		if (fs.lstatSync(lockPath).ino !== owner.inode || fs.readFileSync(lockPath, "utf-8") !== owner.content)
			throw new Error("update lock ownership changed");
		fs.unlinkSync(lockPath);
	} catch (error) {
		cleanupError = error;
	}
	resetCache();
	if (actionError !== undefined) {
		if (cleanupError !== undefined)
			throw new AggregateError([actionError, cleanupError], "autonomy update and lock cleanup failed");
		throw actionError;
	}
	if (cleanupError !== undefined)
		throw new AggregateError([cleanupError], `autonomy.yaml: failed to release update lock: ${lockPath}`);
	return value as T;
}

function cleanupOrphanedPreparedFiles(lockPath: string): void {
	const dir = path.dirname(lockPath);
	const base = path.basename(lockPath);
	for (const name of fs.readdirSync(dir)) {
		if (!name.startsWith(`${base}.owner.`) && !name.startsWith(`${base}.reclaim.owner.`)) continue;
		const candidate = path.join(dir, name);
		let stat: fs.Stats;
		let pid = Number.NaN;
		try {
			stat = fs.lstatSync(candidate);
			const owner = JSON.parse(fs.readFileSync(candidate, "utf-8")) as { pid?: unknown };
			if (typeof owner.pid === "number") pid = owner.pid;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;
			try {
				stat = fs.lstatSync(candidate);
			} catch (statError) {
				if ((statError as NodeJS.ErrnoException).code === "ENOENT") continue;
				throw statError;
			}
			if (Date.now() - stat.mtimeMs < INCOMPLETE_LOCK_STALE_MS) continue;
		}
		if (Number.isInteger(pid) && pid > 0) {
			try {
				process.kill(pid, 0);
				continue;
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== "ESRCH") continue;
			}
		}
		try {
			const current = fs.lstatSync(candidate);
			if (current.ino === stat!.ino) fs.unlinkSync(candidate);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
		}
	}
}

function reclaimStaleLock(lockPath: string): boolean {
	let stat: fs.Stats;
	let content: string;
	try {
		stat = fs.lstatSync(lockPath);
		content = fs.readFileSync(lockPath, "utf-8");
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return true;
		throw error;
	}
	let pid = Number(content);
	let token = `legacy-${stat.ino}`;
	try {
		const owner = JSON.parse(content) as { pid?: unknown; token?: unknown };
		if (typeof owner.pid === "number") pid = owner.pid;
		if (typeof owner.token === "string") token = owner.token;
	} catch {}
	if (Number.isInteger(pid) && pid > 0) {
		try {
			process.kill(pid, 0);
			return false;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ESRCH") return false;
		}
	} else if (Date.now() - stat.mtimeMs < INCOMPLETE_LOCK_STALE_MS) {
		return false;
	}
	const claimPath = `${lockPath}.reclaim`;
	const claimToken = randomUUID();
	const claimContent = JSON.stringify({ pid: process.pid, token: claimToken, lockToken: token });
	const preparedClaim = `${claimPath}.owner.${process.pid}.${claimToken}`;
	fs.writeFileSync(preparedClaim, claimContent, { encoding: "utf-8", flag: "wx", mode: 0o600 });
	let claimInode: number | undefined;
	let claimLinkError: unknown;
	try {
		fs.linkSync(preparedClaim, claimPath);
		claimInode = fs.lstatSync(claimPath).ino;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "EEXIST") claimLinkError = error;
	}
	let preparedClaimCleanupError: unknown;
	try {
		fs.unlinkSync(preparedClaim);
	} catch (error) {
		preparedClaimCleanupError = error;
	}
	if (claimLinkError !== undefined || preparedClaimCleanupError !== undefined) {
		let rollbackError: unknown;
		if (claimInode !== undefined) {
			try {
				if (fs.lstatSync(claimPath).ino !== claimInode || fs.readFileSync(claimPath, "utf-8") !== claimContent)
					throw new Error("reclaim claim ownership changed during setup rollback");
				fs.unlinkSync(claimPath);
			} catch (error) {
				rollbackError = error;
			}
		}
		throw new AggregateError(
			[claimLinkError, preparedClaimCleanupError, rollbackError].filter((error) => error !== undefined),
			"autonomy update reclaim setup failed",
		);
	}
	if (claimInode === undefined) return reclaimDeadClaim(lockPath, claimPath);
	const snapshotPath = `${lockPath}.reclaim-snapshot.${claimToken}`;
	let reclaimed = false;
	let operationError: unknown;
	try {
		fs.linkSync(lockPath, snapshotPath);
		const snapshotStat = fs.lstatSync(snapshotPath);
		if (
			snapshotStat.ino === stat.ino &&
			fs.readFileSync(snapshotPath, "utf-8") === content &&
			fs.lstatSync(lockPath).ino === snapshotStat.ino &&
			fs.readFileSync(lockPath, "utf-8") === content
		) {
			fs.unlinkSync(lockPath);
			reclaimed = true;
		}
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") operationError = error;
	}
	const cleanupErrors: unknown[] = [];
	try {
		fs.unlinkSync(snapshotPath);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") cleanupErrors.push(error);
	}
	try {
		if (fs.lstatSync(claimPath).ino !== claimInode || fs.readFileSync(claimPath, "utf-8") !== claimContent)
			throw new Error("reclaim claim ownership changed");
		fs.unlinkSync(claimPath);
	} catch (error) {
		cleanupErrors.push(error);
	}
	if (operationError !== undefined || cleanupErrors.length > 0)
		throw new AggregateError(
			[operationError, ...cleanupErrors].filter((error) => error !== undefined),
			"autonomy update lock reclamation failed",
		);
	return reclaimed;
}

function reclaimDeadClaim(lockPath: string, claimPath: string): boolean {
	let stat: fs.Stats;
	let content: string;
	try {
		stat = fs.lstatSync(claimPath);
		content = fs.readFileSync(claimPath, "utf-8");
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return true;
		throw error;
	}
	let pid = Number.NaN;
	let token: string | undefined;
	try {
		const owner = JSON.parse(content) as { pid?: unknown; token?: unknown };
		if (typeof owner.pid === "number") pid = owner.pid;
		if (typeof owner.token === "string") token = owner.token;
	} catch {}
	if (Number.isInteger(pid) && pid > 0) {
		try {
			process.kill(pid, 0);
			return false;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ESRCH") return false;
		}
	} else if (Date.now() - stat.mtimeMs < INCOMPLETE_LOCK_STALE_MS) {
		return false;
	}
	if (token) {
		try {
			fs.unlinkSync(`${lockPath}.reclaim-snapshot.${token}`);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
		}
	}
	try {
		if (fs.lstatSync(claimPath).ino !== stat.ino || fs.readFileSync(claimPath, "utf-8") !== content) return false;
		fs.unlinkSync(claimPath);
		return true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return true;
		throw error;
	}
}

function writeAutonomyConfigUnlocked(configPath: string, config: AutonomyConfig): void {
	const cleaned = cleanConfig(config);
	if (Object.keys(cleaned.edges).length === 0 && Object.keys(cleaned.publications ?? {}).length === 0) {
		try {
			fs.unlinkSync(configPath);
		} catch (err: unknown) {
			if ((err as NodeJS.ErrnoException).code !== "ENOENT") throw err;
		}
		return;
	}
	const tmpPath = path.join(
		path.dirname(configPath),
		`.${path.basename(configPath)}.${process.pid}.${randomUUID()}.tmp`,
	);
	try {
		fs.writeFileSync(tmpPath, stringifyYaml(cleaned), { encoding: "utf-8", flag: "wx", mode: 0o600 });
		fs.renameSync(tmpPath, configPath);
	} finally {
		try {
			fs.unlinkSync(tmpPath);
		} catch {}
	}
}

function cleanConfig(config: AutonomyConfig): AutonomyConfig {
	const edges: AutonomyConfig["edges"] = {};
	for (const [source, targets] of Object.entries(config.edges)) {
		const filtered: Record<string, NonNullable<EdgeSetting>> = {};
		for (const [target, setting] of Object.entries(targets)) {
			if (VALID_SETTINGS.has(setting)) {
				filtered[target] = setting;
			}
		}
		if (Object.keys(filtered).length > 0) {
			edges[source] = filtered;
		}
	}
	const publications = parsePublicationMap<PublicationOverride>(config.publications, VALID_PUBLICATION_OVERRIDES);
	return Object.keys(publications).length > 0 ? { edges, publications } : { edges };
}

export function resetCache(): void {
	cache = null;
}
