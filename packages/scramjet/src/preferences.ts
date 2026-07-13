import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";

export interface Preferences {
	title_indicator: boolean;
	bell: boolean;
}

export const DEFAULT_PREFERENCES: Readonly<Preferences> = { title_indicator: true, bell: false };

let cache: { path: string; mtimeMs: number; prefs: Preferences } | null = null;

export function defaultPreferencesPath(): string {
	const configHome = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config");
	return path.join(configHome, "scramjet", "preferences.yaml");
}

export function loadPreferences(configPath: string): Preferences {
	let stat: fs.Stats;
	try {
		stat = fs.statSync(configPath);
	} catch (err: unknown) {
		if (cache?.path === configPath) cache = null;
		if ((err as NodeJS.ErrnoException).code === "ENOENT") return { ...DEFAULT_PREFERENCES };
		const msg = err instanceof Error ? err.message : String(err);
		throw new Error(`preferences.yaml: cannot stat config file: ${msg}`);
	}

	if (cache?.path === configPath && stat.mtimeMs === cache.mtimeMs) {
		return { ...cache.prefs };
	}

	try {
		const raw = fs.readFileSync(configPath, "utf-8");
		const doc = parseYaml(raw);
		const prefs = parsePreferences(doc);
		cache = { path: configPath, mtimeMs: stat.mtimeMs, prefs };
		return { ...prefs };
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		throw new Error(`preferences.yaml: failed to load config: ${msg}`);
	}
}

function parsePreferences(doc: unknown): Preferences {
	if (doc == null || typeof doc !== "object" || Array.isArray(doc)) {
		return { ...DEFAULT_PREFERENCES };
	}
	const obj = doc as Record<string, unknown>;
	return {
		title_indicator:
			typeof obj.title_indicator === "boolean" ? obj.title_indicator : DEFAULT_PREFERENCES.title_indicator,
		bell: typeof obj.bell === "boolean" ? obj.bell : DEFAULT_PREFERENCES.bell,
	};
}

export function savePreferences(configPath: string, prefs: Preferences): void {
	const dir = path.dirname(configPath);
	fs.mkdirSync(dir, { recursive: true });
	const tmpPath = `${configPath}.tmp`;
	try {
		fs.writeFileSync(tmpPath, stringifyYaml({ title_indicator: prefs.title_indicator, bell: prefs.bell }), "utf-8");
		try {
			fs.renameSync(tmpPath, configPath);
		} catch (err: unknown) {
			try {
				fs.unlinkSync(tmpPath);
			} catch {}
			throw err;
		}
	} finally {
		resetCache();
	}
}

export function resetCache(): void {
	cache = null;
}
