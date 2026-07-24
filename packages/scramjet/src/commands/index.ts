import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { parseAutonomyRecommendations, validateRecommendations } from "../autonomy-settings.js";
import { packageRoot } from "../docs-registry.js";
import type { ScramjetState } from "../types.js";
import { ensureAgentBridge } from "./agent-bridge.js";
import { buildAgentRegistry, buildRegistry, type FileEntry } from "./loader.js";

// Stage 9 / F6,F7,F19: filesystem-discovery error handling. These helpers
// used to swallow all errors silently; now they collect human-readable
// warnings through an out-parameter so the `resources_discover` hook can
// surface them through the Scramjet logger. ENOENT is still treated as
// "absent and fine" — only unexpected errors (EACCES, EIO, …) are reported.
function safeReaddir(dir: string, warnings: string[]): { name: string; isDirectory: boolean }[] {
	let raw: import("node:fs").Dirent[];
	try {
		raw = readdirSync(dir, { withFileTypes: true }) as import("node:fs").Dirent[];
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		if (code !== "ENOENT") {
			warnings.push(`[scramjet/discovery] could not scan ${dir} (${code ?? "unknown"}: ${(err as Error).message})`);
		}
		return [];
	}
	return raw.map((e) => {
		const name = String(e.name);
		let isDirectory = e.isDirectory();
		if (e.isSymbolicLink()) {
			try {
				isDirectory = statSync(join(dir, name)).isDirectory();
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				if (code === "ENOENT") {
					warnings.push(
						`[scramjet/discovery] symlink ${join(dir, name)} has a missing target; if you migrated from the single-repo layout, remove the old symlink and re-run: ln -sfn "$(pwd)/packages/scramjet/mach12" "${join(dir, name)}"`,
					);
				} else {
					warnings.push(
						`[scramjet/discovery] could not stat symlink ${join(dir, name)} (${code ?? "unknown"}: ${(err as Error).message}); treating as non-directory`,
					);
				}
				isDirectory = false;
			}
		}
		return { name, isDirectory };
	});
}

function collectEntries(rootDir: string, scope: "global" | "project", subdir: string, warnings: string[]): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const setEntry of safeReaddir(rootDir, warnings)) {
		if (!setEntry.isDirectory) continue;
		const setName = setEntry.name;
		const dir = join(rootDir, setName, subdir);
		for (const fileEntry of safeReaddir(dir, warnings)) {
			if (fileEntry.isDirectory) continue;
			if (!fileEntry.name.endsWith(".md")) continue;
			const filePath = join(dir, fileEntry.name);
			let content: string;
			try {
				content = readFileSync(filePath, "utf-8");
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				warnings.push(
					`[scramjet/discovery] could not read ${filePath} (${code ?? "unknown"}: ${(err as Error).message}); skipping`,
				);
				continue;
			}
			entries.push({ filePath, content, setName, scope });
		}
	}
	return entries;
}

export function commandFingerprint(content: string): string {
	return createHash("sha256").update(content).digest("hex").slice(0, 12);
}

export function normalizedCommandSource(scope: "global" | "project", setName: string, filePath: string): string {
	const relative = `${setName}/commands/${basename(filePath)}`;
	return scope === "project" ? `.scramjet/${relative}` : relative;
}

function globalRoot(): string {
	// Mirror scripts/postinstall.js: honor SCRAMJET_CACHE (explicit override),
	// else XDG_DATA_HOME (XDG spec), else ~/.local/share/scramjet. If the
	// seeder and the loader disagree on this path, every postinstall write
	// lands somewhere the runtime can't see. (F3)
	return (
		process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "scramjet")
	);
}

export function registerCommandLoader(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("resources_discover", (event) => {
		// F19: the hook body calls only pure functions today, but a stray
		// crash here would prevent Pi from completing resource discovery for
		// the whole session. Wrap once at the top level so any unexpected
		// failure degrades to "no scramjet commands this session" with a
		// loud warning, rather than a hard startup crash.
		//
		// themePaths depends only on packageRoot() (pure path work), so it is
		// built outside the try and returned in both branches: a command-discovery
		// failure must not also drop the bundled scramjet-dark theme (F1).
		const themePaths = [join(packageRoot(), "themes")];
		try {
			const discoveryWarnings: string[] = [];
			const globalDir = globalRoot();
			const projectDir = join(event.cwd, ".scramjet");

			const globalEntries = collectEntries(globalDir, "global", "commands", discoveryWarnings);
			const projectEntries = collectEntries(projectDir, "project", "commands", discoveryWarnings);
			const commandEntries = [...globalEntries, ...projectEntries];
			const { registry, warnings } = buildRegistry(commandEntries);
			state.registry = registry;
			const entriesByPath = new Map(commandEntries.map((entry) => [entry.filePath, entry]));
			for (const def of registry.values()) {
				const entry = entriesByPath.get(def.filePath);
				if (!entry) continue;
				state.logger.debug("discovery", "command discovered", {
					command: def.name,
					scope: entry.scope,
					source: normalizedCommandSource(entry.scope, entry.setName, entry.filePath),
					fingerprint: commandFingerprint(entry.content),
				});
			}

			const globalAgentEntries = collectEntries(globalDir, "global", "agents", discoveryWarnings);
			const projectAgentEntries = collectEntries(projectDir, "project", "agents", discoveryWarnings);
			const { agentRegistry, warnings: agentWarnings } = buildAgentRegistry([
				...globalAgentEntries,
				...projectAgentEntries,
			]);
			state.agentRegistry = agentRegistry;

			const recommendations = new Map<string, import("../types.js").AutonomyRecommendations>();
			for (const rootDir of [globalDir, projectDir]) {
				for (const setEntry of safeReaddir(rootDir, discoveryWarnings)) {
					if (!setEntry.isDirectory) continue;
					const recPath = join(rootDir, setEntry.name, "autonomy-defaults.yaml");
					let content: string;
					try {
						content = readFileSync(recPath, "utf-8");
					} catch (err) {
						const code = (err as NodeJS.ErrnoException).code;
						if (code !== "ENOENT") {
							discoveryWarnings.push(
								`[scramjet/discovery] could not read ${recPath} (${code ?? "unknown"}: ${(err as Error).message})`,
							);
						}
						continue;
					}
					try {
						const recs = parseAutonomyRecommendations(content, discoveryWarnings);
						if (Object.keys(recs.edges).length > 0) {
							for (const w of validateRecommendations(recs, registry)) {
								discoveryWarnings.push(w);
							}
							recommendations.set(setEntry.name, recs);
						}
					} catch (err) {
						discoveryWarnings.push(`[scramjet/discovery] could not parse ${recPath}: ${(err as Error).message}`);
					}
				}
			}
			state.autonomyRecommendations = recommendations;

			const bridge = ensureAgentBridge(agentRegistry, [globalDir, projectDir]);
			if (bridge.created.length > 0 && bridge.targetDir !== null) {
				state.logger.debug("discovery", `bridged ${bridge.created.length} agent(s) into ${bridge.targetDir}`);
			}
			if (bridge.pruned.length > 0 && bridge.targetDir !== null) {
				state.logger.debug(
					"discovery",
					`pruned ${bridge.pruned.length} stale agent symlink(s) from ${bridge.targetDir}`,
				);
			}

			for (const warning of discoveryWarnings) {
				state.logger.warn("discovery", warning);
			}
			for (const warning of [...warnings, ...agentWarnings, ...bridge.warnings]) {
				state.logger.warn("discovery", warning);
			}
			const promptPaths: string[] = [];
			for (const def of registry.values()) promptPaths.push(def.filePath);
			return { promptPaths, themePaths };
		} catch (err) {
			state.logger.warn(
				"discovery",
				`failed: ${(err as Error).message}; no scramjet commands will be available this session (bundled scramjet-dark theme unaffected)`,
			);
			return { promptPaths: [], themePaths };
		}
	});
}
