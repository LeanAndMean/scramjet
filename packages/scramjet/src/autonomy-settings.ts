import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import type { AutonomyConfig, CommandRegistry, EdgeSetting } from "./types.js";

const VALID_SETTINGS = new Set(["chain", "pause"]);

let cache: { path: string; mtimeMs: number; config: AutonomyConfig | null } | null = null;

export function defaultConfigPath(): string {
	const configHome = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config");
	return path.join(configHome, "scramjet", "autonomy.yaml");
}

export function parseAutonomyConfig(raw: string): AutonomyConfig {
	const doc = parseYaml(raw);
	if (
		doc == null ||
		typeof doc !== "object" ||
		!("edges" in doc) ||
		doc.edges == null ||
		typeof doc.edges !== "object"
	) {
		return { edges: {} };
	}

	const edges: AutonomyConfig["edges"] = {};
	for (const [source, targets] of Object.entries(doc.edges as Record<string, unknown>)) {
		if (targets == null || typeof targets !== "object") continue;
		const targetMap: Record<string, "chain" | "pause"> = {};
		for (const [target, setting] of Object.entries(targets as Record<string, unknown>)) {
			if (typeof setting === "string" && VALID_SETTINGS.has(setting)) {
				targetMap[target] = setting as "chain" | "pause";
			}
		}
		if (Object.keys(targetMap).length > 0) {
			edges[source] = targetMap;
		}
	}
	return { edges };
}

export function loadAutonomyConfig(configPath: string): AutonomyConfig | null {
	let stat: fs.Stats;
	try {
		stat = fs.statSync(configPath);
	} catch (err: unknown) {
		if (cache?.path === configPath) cache = null;
		if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
			const msg = err instanceof Error ? err.message : String(err);
			throw new Error(`autonomy.yaml: cannot stat config file: ${msg}`);
		}
		return null;
	}

	if (cache?.path === configPath && stat.mtimeMs === cache.mtimeMs) {
		return cache.config;
	}

	try {
		const raw = fs.readFileSync(configPath, "utf-8");
		const config = parseAutonomyConfig(raw);
		cache = { path: configPath, mtimeMs: stat.mtimeMs, config };
		return config;
	} catch (err: unknown) {
		cache = { path: configPath, mtimeMs: stat.mtimeMs, config: null };
		const msg = err instanceof Error ? err.message : String(err);
		throw new Error(`autonomy.yaml: failed to load config: ${msg}`);
	}
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

export function validateConfig(config: AutonomyConfig, registry: CommandRegistry): string[] {
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
	return warnings;
}

export function saveAutonomyConfig(configPath: string, config: AutonomyConfig): void {
	const cleaned = cleanConfig(config);
	const dir = path.dirname(configPath);
	fs.mkdirSync(dir, { recursive: true });
	const tmpPath = `${configPath}.tmp`;
	try {
		if (Object.keys(cleaned.edges).length === 0) {
			try {
				fs.unlinkSync(configPath);
			} catch (err: unknown) {
				if ((err as NodeJS.ErrnoException).code !== "ENOENT") throw err;
			}
		} else {
			const yaml = stringifyYaml(cleaned);
			fs.writeFileSync(tmpPath, yaml, "utf-8");
			try {
				fs.renameSync(tmpPath, configPath);
			} catch (err: unknown) {
				try {
					fs.unlinkSync(tmpPath);
				} catch {}
				throw err;
			}
		}
	} finally {
		resetCache();
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
	return { edges };
}

export function resetCache(): void {
	cache = null;
}
