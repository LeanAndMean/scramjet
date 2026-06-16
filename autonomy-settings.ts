import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parse as parseYaml } from "yaml";
import type { AutonomyConfig, CommandRegistry, EdgeSetting } from "./types.ts";

const VALID_SETTINGS = new Set(["chain", "pause"]);

let cachedMtimeMs = -1;
let cachedConfig: AutonomyConfig | null = null;
let cachedConfigPath = "";

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
	} catch {
		if (cachedConfigPath === configPath) {
			cachedMtimeMs = -1;
			cachedConfig = null;
		}
		return null;
	}

	if (cachedConfigPath === configPath && stat.mtimeMs === cachedMtimeMs) {
		return cachedConfig;
	}

	const raw = fs.readFileSync(configPath, "utf-8");
	const config = parseAutonomyConfig(raw);
	cachedConfigPath = configPath;
	cachedMtimeMs = stat.mtimeMs;
	cachedConfig = config;
	return config;
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

export function resetCache(): void {
	cachedMtimeMs = -1;
	cachedConfig = null;
	cachedConfigPath = "";
}
