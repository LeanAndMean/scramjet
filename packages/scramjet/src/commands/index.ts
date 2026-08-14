import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, type Stats, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { parseAutonomyRecommendations, validateRecommendations } from "../autonomy-settings.js";
import { packageRoot } from "../docs-registry.js";
import type { ScramjetState } from "../types.js";
import { ensureAgentBridge } from "./agent-bridge.js";
import { buildAgentRegistry, buildRegistry, type FileEntry } from "./loader.js";

type Scope = "global" | "project";
type SelectedSet = { name: string; dir: string; scope: Scope; source: "destination" | "package" | "project" };
type Inspection = {
	lstat(path: string): Stats;
	stat(path: string): Stats;
	readdir(path: string): string[];
};

const BUNDLED_SETS = ["mach12", "scramjet"] as const;

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

function enumerateSets(root: string, scope: Scope, source: SelectedSet["source"], warnings: string[]): SelectedSet[] {
	return safeReaddir(root, warnings)
		.filter((entry) => entry.isDirectory && (scope === "project" || !BUNDLED_SETS.includes(entry.name as any)))
		.map((entry) => ({ name: entry.name, dir: join(root, entry.name), scope, source }));
}

function collectEntries(sets: SelectedSet[], subdir: string, warnings: string[]): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const set of sets) {
		const dir = join(set.dir, subdir);
		for (const fileEntry of safeReaddir(dir, warnings)) {
			if (fileEntry.isDirectory || !fileEntry.name.endsWith(".md")) continue;
			const filePath = join(dir, fileEntry.name);
			try {
				entries.push({ filePath, content: readFileSync(filePath, "utf-8"), setName: set.name, scope: set.scope });
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				warnings.push(
					`[scramjet/discovery] could not read ${filePath} (${code ?? "unknown"}: ${(err as Error).message}); skipping`,
				);
			}
		}
	}
	return entries;
}

function selectBundledSet(
	name: (typeof BUNDLED_SETS)[number],
	globalDir: string,
	bundledRoot: string,
	inspection: Inspection,
	warnings: string[],
): { set?: SelectedSet; diagnostic?: string; classification: string } {
	const destination = join(globalDir, name);
	try {
		inspection.lstat(destination);
		return { set: { name, dir: destination, scope: "global", source: "destination" }, classification: "destination" };
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		if (code !== "ENOENT") {
			const diagnostic = `could not inspect bundled command-set destination ${destination} (${code ?? "unknown"}: ${(err as Error).message}); package fallback was not used`;
			warnings.push(`[scramjet/discovery] ${diagnostic}`);
			return { diagnostic, classification: `destination-${code ?? "unknown"}` };
		}
	}

	const source = join(bundledRoot, name);
	try {
		if (!inspection.stat(source).isDirectory()) {
			const diagnostic = `bundled package source ${source} is not a directory; ${destination} remains unavailable`;
			warnings.push(`[scramjet/discovery] ${diagnostic}`);
			return { diagnostic, classification: "source-not-directory" };
		}
		if (inspection.readdir(source).length === 0) {
			const diagnostic = `bundled package source ${source} is empty; ${destination} remains unavailable`;
			warnings.push(`[scramjet/discovery] ${diagnostic}`);
			return { diagnostic, classification: "source-empty" };
		}
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		const diagnostic = `could not use bundled package source ${source} (${code ?? "unknown"}: ${(err as Error).message}); ${destination} remains unavailable`;
		warnings.push(`[scramjet/discovery] ${diagnostic}`);
		return { diagnostic, classification: `source-${code ?? "unknown"}` };
	}

	const diagnostic = `bundled destination ${destination} is missing; using packaged ${name} command set read-only from ${source}. To restore durable editable copies, run node "${join(bundledRoot, "scripts", "postinstall.js")}" and restart Scramjet.`;
	warnings.push(`[scramjet/discovery] ${diagnostic}`);
	return {
		set: { name, dir: source, scope: "global", source: "package" },
		diagnostic,
		classification: "fallback",
	};
}

export function commandFingerprint(content: string): string {
	return createHash("sha256").update(content).digest("hex").slice(0, 12);
}

export function normalizedCommandSource(scope: Scope, setName: string, filePath: string): string {
	const relative = `${setName}/commands/${basename(filePath)}`;
	return scope === "project" ? `.scramjet/${relative}` : relative;
}

function globalRoot(): string {
	return (
		process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "scramjet")
	);
}

export function registerCommandLoader(
	pi: ExtensionAPI,
	state: ScramjetState,
	dependencies: { bundledRoot?: string; inspection?: Partial<Inspection>; interactiveOutput?: boolean } = {},
): void {
	const bundledRoot = dependencies.bundledRoot ?? packageRoot();
	const interactiveOutput = dependencies.interactiveOutput ?? Boolean(process.stdout.isTTY);
	const inspection: Inspection = {
		lstat: dependencies.inspection?.lstat ?? lstatSync,
		stat: dependencies.inspection?.stat ?? statSync,
		readdir: dependencies.inspection?.readdir ?? readdirSync,
	};
	let lastNotificationSignature = "";

	pi.on("resources_discover", (event, ctx) => {
		const themePaths = [join(packageRoot(), "themes")];
		try {
			const discoveryWarnings: string[] = [];
			const globalDir = globalRoot();
			const projectDir = join(event.cwd, ".scramjet");
			const bundled = BUNDLED_SETS.map((name) =>
				selectBundledSet(name, globalDir, bundledRoot, inspection, discoveryWarnings),
			);
			const selectedSets = [
				...bundled.flatMap((result) => (result.set ? [result.set] : [])),
				...enumerateSets(globalDir, "global", "destination", discoveryWarnings),
				...enumerateSets(projectDir, "project", "project", discoveryWarnings),
			];

			const commandEntries = collectEntries(selectedSets, "commands", discoveryWarnings);
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

			const { agentRegistry, warnings: agentWarnings } = buildAgentRegistry(
				collectEntries(selectedSets, "agents", discoveryWarnings),
			);
			state.agentRegistry = agentRegistry;

			const recommendations = new Map<string, import("../types.js").AutonomyRecommendations>();
			for (const set of selectedSets) {
				const recPath = join(set.dir, "autonomy-defaults.yaml");
				let content: string;
				try {
					content = readFileSync(recPath, "utf-8");
				} catch (err) {
					const code = (err as NodeJS.ErrnoException).code;
					if (code !== "ENOENT")
						discoveryWarnings.push(
							`[scramjet/discovery] could not read ${recPath} (${code ?? "unknown"}: ${(err as Error).message})`,
						);
					continue;
				}
				try {
					const recs = parseAutonomyRecommendations(content, discoveryWarnings);
					if (Object.keys(recs.edges).length > 0) {
						for (const warning of validateRecommendations(recs, registry)) discoveryWarnings.push(warning);
						recommendations.set(set.name, recs);
					}
				} catch (err) {
					discoveryWarnings.push(`[scramjet/discovery] could not parse ${recPath}: ${(err as Error).message}`);
				}
			}
			state.autonomyRecommendations = recommendations;

			const bridge = ensureAgentBridge(agentRegistry, [
				globalDir,
				projectDir,
				...BUNDLED_SETS.map((name) => join(bundledRoot, name)),
			]);
			if (bridge.created.length > 0 && bridge.targetDir !== null)
				state.logger.debug("discovery", `bridged ${bridge.created.length} agent(s) into ${bridge.targetDir}`);
			if (bridge.pruned.length > 0 && bridge.targetDir !== null)
				state.logger.debug(
					"discovery",
					`pruned ${bridge.pruned.length} stale agent symlink(s) from ${bridge.targetDir}`,
				);

			for (const warning of discoveryWarnings) state.logger.warn("discovery", warning);
			for (const warning of [...warnings, ...agentWarnings, ...bridge.warnings])
				state.logger.warn("discovery", warning);

			const visibleDiagnostics = bundled.flatMap((result) => (result.diagnostic ? [result.diagnostic] : []));
			const signature = bundled.map((result) => result.classification).join("|");
			if (visibleDiagnostics.length > 0) {
				const message = visibleDiagnostics.join("\n");
				if (ctx?.hasUI && interactiveOutput && signature !== lastNotificationSignature)
					ctx.ui.notify(message, "warning");
				else if (ctx?.hasUI && !interactiveOutput) process.stderr.write(`[scramjet/discovery] ${message}\n`);
			}
			lastNotificationSignature = visibleDiagnostics.length > 0 ? signature : "";

			return { promptPaths: [...registry.values()].map((def) => def.filePath), themePaths };
		} catch (err) {
			state.logger.warn(
				"discovery",
				`failed: ${(err as Error).message}; no scramjet commands will be available this session (bundled scramjet-dark theme unaffected)`,
			);
			return { promptPaths: [], themePaths };
		}
	});
}
